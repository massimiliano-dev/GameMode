//! Freezes the game while GameMode's overlay is open. Games read the
//! controller even without focus, so moving through the overlay would also
//! move the character. The game is frozen like Quick Resume does (audio
//! muted, every process stopped) but not minimized: it stays visible behind
//! the overlay. Games with an anti-cheat keep running, since freezing them
//! gets players kicked.

use std::sync::Mutex;

use sysinfo::{Pid, ProcessesToUpdate, System};
use tauri::{AppHandle, Manager};

use crate::launcher::Phase;
use crate::AppState;

/// Processes frozen for the overlay. The lock is held while freezing, so a
/// close arriving meanwhile waits for it and thaws what was frozen.
static FROZEN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Whether a session may be frozen while the overlay is open.
pub fn wanted(enabled: bool, phase: Phase, anti_cheat: bool, game_id: &str) -> bool {
    enabled && phase == Phase::Running && !anti_cheat && !game_id.starts_with("app:")
}

/// Freezes the running game, in the background so the overlay shows at once.
pub fn begin(app: &AppHandle) {
    let state = app.state::<AppState>();
    let enabled = state.db.lock().settings.overlay_pause;
    let Some(session) = state.launcher.current() else { return };
    if !wanted(enabled, session.phase, session.anti_cheat, &session.game_id) {
        return;
    }
    let pids = state.launcher.running_pids();
    std::thread::spawn(move || {
        let mut frozen = FROZEN.lock().unwrap_or_else(|e| e.into_inner());
        // Closed again before we got here, or already frozen.
        if !frozen.is_empty() || !crate::overlay::is_open() {
            return;
        }
        let family = crate::freeze::family(&pids);
        match crate::freeze::suspend(&family, None) {
            Ok(()) => {
                let marked = with_start(&family);
                crate::live::update(|s| s.overlay_frozen = marked);
                *frozen = family;
            }
            Err(err) => log::info!("overlay: game keeps running ({err:#})"),
        }
    });
}

/// Thaws the game when the overlay closes.
pub fn end() {
    std::thread::spawn(thaw_now);
}

/// Thaws at once: before Quick Resume freezes the game for real, and before
/// GameMode quits or the PC shuts down.
pub fn thaw_now() {
    let mut frozen = FROZEN.lock().unwrap_or_else(|e| e.into_inner());
    if frozen.is_empty() {
        return;
    }
    let pids = std::mem::take(&mut *frozen);
    if let Err(err) = crate::freeze::resume(&pids) {
        log::warn!("overlay: {err:#}");
    }
    crate::live::update(|s| s.overlay_frozen.clear());
}

/// Thaws a game a crashed GameMode left frozen behind its overlay.
pub fn recover() {
    let marked = crate::live::snapshot().overlay_frozen;
    if marked.is_empty() {
        return;
    }
    thaw_marked(&marked);
    crate::live::update(|s| s.overlay_frozen.clear());
}

/// Resumes the processes in `marked` that are still the same ones (same
/// start time): after a reboot the pids may belong to something else.
pub fn thaw_marked(marked: &[(u32, u64)]) {
    let now = with_start(&marked.iter().map(|(pid, _)| *pid).collect::<Vec<_>>());
    let same = still_same(marked, &now);
    if !same.is_empty() {
        if let Err(err) = crate::freeze::resume(&same) {
            log::warn!("overlay: {err:#}");
        }
    }
}

/// The pids of `marked` that `now` reports with the same start time.
pub fn still_same(marked: &[(u32, u64)], now: &[(u32, u64)]) -> Vec<u32> {
    marked.iter().filter(|m| now.contains(m)).map(|(pid, _)| *pid).collect()
}

fn with_start(pids: &[u32]) -> Vec<(u32, u64)> {
    let mut sys = System::new();
    let targets: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
    sys.refresh_processes(ProcessesToUpdate::Some(&targets), true);
    pids.iter().filter_map(|&pid| sys.process(Pid::from_u32(pid)).map(|p| (pid, p.start_time()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_running_games_are_frozen() {
        assert!(wanted(true, Phase::Running, false, "steam:570"));
        assert!(!wanted(false, Phase::Running, false, "steam:570"));
        assert!(!wanted(true, Phase::Launching, false, "steam:570"));
        assert!(!wanted(true, Phase::Running, true, "steam:570"));
        assert!(!wanted(true, Phase::Running, false, "app:Spotify"));
    }

    #[test]
    fn a_reused_pid_is_not_thawed() {
        let marked = [(10, 100), (11, 200)];
        let now = [(10, 100), (11, 999)];
        assert_eq!(still_same(&marked, &now), vec![10]);
    }
}
