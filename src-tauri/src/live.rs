//! What survives a crash or a restart of GameMode: the games it is following
//! (running or suspended) and the power plan it changed. Kept in its own
//! small file, `live_session.json`, so the heartbeat never rewrites the
//! library. At startup `launcher::reattach` reads it back: a game still
//! alive is followed again, a dead one gets its play time recorded.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

const FILE: &str = "live_session.json";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiveSession {
    pub game_id: String,
    /// Unix seconds when the game was seen running (play time counts from here).
    pub started_at: i64,
    /// Unix seconds of the last heartbeat.
    pub last_seen: i64,
    /// `(pid, start time)` of the game's processes; the start time tells a
    /// reused pid from the old process.
    pub pids: Vec<(u32, u64)>,
    pub suspended: bool,
    /// Seconds spent suspended before `last_seen`, not play time.
    #[serde(default)]
    pub paused_secs: u64,
}

impl LiveSession {
    /// Play time up to the last heartbeat.
    pub fn played(&self) -> u64 {
        ((self.last_seen - self.started_at).max(0) as u64).saturating_sub(self.paused_secs)
    }

    /// Whether any of the remembered processes is still the same one.
    pub fn alive(&self, start_time_of: impl Fn(u32) -> Option<u64>) -> bool {
        self.pids.iter().any(|(pid, start)| start_time_of(*pid) == Some(*start))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LiveState {
    pub sessions: Vec<LiveSession>,
    /// The power plan active before performance mode switched it.
    pub previous_power_scheme: Option<String>,
}

impl LiveState {
    pub fn upsert(&mut self, session: LiveSession) {
        match self.sessions.iter_mut().find(|s| s.game_id == session.game_id) {
            Some(slot) => *slot = session,
            None => self.sessions.push(session),
        }
    }

    pub fn remove(&mut self, game_id: &str) {
        self.sessions.retain(|s| s.game_id != game_id);
    }

    pub fn get_mut(&mut self, game_id: &str) -> Option<&mut LiveSession> {
        self.sessions.iter_mut().find(|s| s.game_id == game_id)
    }
}

static PATH: OnceLock<PathBuf> = OnceLock::new();
static STATE: Mutex<Option<LiveState>> = Mutex::new(None);

/// Sets where the file lives; what it holds is read on first use.
pub fn init(data_dir: &Path) {
    let _ = PATH.set(data_dir.join(FILE));
}

pub fn file_in(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

pub fn read_from(path: &Path) -> LiveState {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn write_to(path: &Path, state: &LiveState) {
    let empty = state.sessions.is_empty() && state.previous_power_scheme.is_none();
    let result = if empty {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        serde_json::to_string_pretty(state).map_err(std::io::Error::other).and_then(|json| {
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, json)?;
            std::fs::rename(&tmp, path)
        })
    };
    if let Err(err) = result {
        eprintln!("saving live session failed: {err}");
    }
}

/// Runs `f` on the state and persists it; a no-op before `init`.
pub fn update<T>(f: impl FnOnce(&mut LiveState) -> T) -> Option<T> {
    let path = PATH.get()?;
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(|| read_from(path));
    let out = f(state);
    write_to(path, state);
    Some(out)
}

/// A copy of the current state.
pub fn snapshot() -> LiveState {
    let Some(path) = PATH.get() else { return LiveState::default() };
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_insert_with(|| read_from(path)).clone()
}

pub fn power_scheme() -> Option<String> {
    snapshot().previous_power_scheme
}

pub fn set_power_scheme(scheme: Option<String>) {
    update(|s| s.previous_power_scheme = scheme);
}

pub fn take_power_scheme() -> Option<String> {
    update(|s| s.previous_power_scheme.take()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str) -> LiveSession {
        LiveSession { game_id: id.into(), started_at: 1000, last_seen: 1600, pids: vec![(42, 7), (43, 9)], suspended: false, paused_secs: 100 }
    }

    #[test]
    fn roundtrips_through_json() {
        let mut state = LiveState::default();
        state.upsert(session("steam:1"));
        state.previous_power_scheme = Some("381b4222-f694-41f0-9685-ff5bb260df2e".into());
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(serde_json::from_str::<LiveState>(&json).unwrap(), state);
        assert_eq!(read_from(Path::new("/nonexistent/live_session.json")), LiveState::default());
    }

    #[test]
    fn upsert_replaces_the_same_game() {
        let mut state = LiveState::default();
        state.upsert(session("steam:1"));
        state.upsert(LiveSession { suspended: true, ..session("steam:1") });
        state.upsert(session("xbox:2"));
        assert_eq!(state.sessions.len(), 2);
        assert!(state.sessions[0].suspended);
        state.remove("steam:1");
        assert_eq!(state.sessions.len(), 1);
    }

    #[test]
    fn alive_only_when_a_pid_kept_its_start_time() {
        let s = session("steam:1");
        assert!(s.alive(|pid| (pid == 43).then_some(9)));
        // Same pid, reused by another process.
        assert!(!s.alive(|pid| (pid == 42).then_some(8)));
        assert!(!s.alive(|_| None));
    }

    #[test]
    fn played_excludes_pauses() {
        assert_eq!(session("x").played(), 500);
        assert_eq!(LiveSession { paused_secs: 5000, ..session("x") }.played(), 0);
    }

    #[test]
    fn writes_and_removes_the_file() {
        let dir = std::env::temp_dir().join(format!("gamemode-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = file_in(&dir);
        let mut state = LiveState::default();
        state.upsert(session("steam:1"));
        write_to(&path, &state);
        assert_eq!(read_from(&path), state);
        write_to(&path, &LiveState::default());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
