//! Starting games and following them: store clients start the real game
//! process themselves, so a game is "running" while any process whose
//! executable lives inside its install folder is alive.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::model::{now, Game, LaunchSpec, Session, Store};
use crate::{window, AppState};

const POLL: Duration = Duration::from_secs(2);
/// How long a store client may take before the game process shows up.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Games often restart themselves (launcher → game, anti-cheat…).
const EXIT_GRACE: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Launching,
    Running,
    Exited,
    Failed,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub game_id: String,
    pub phase: Phase,
    pub started_at: i64,
    pub seconds: u64,
}

struct Active {
    generation: u64,
    state: SessionState,
    pids: Vec<u32>,
}

#[derive(Default)]
pub struct Launcher {
    active: Mutex<Option<Active>>,
    generation: AtomicU64,
}

impl Launcher {
    pub fn current(&self) -> Option<SessionState> {
        self.active.lock().unwrap().as_ref().map(|a| a.state.clone())
    }

    pub fn running_pids(&self) -> Vec<u32> {
        self.active.lock().unwrap().as_ref().map(|a| a.pids.clone()).unwrap_or_default()
    }

    fn update(&self, generation: u64, f: impl FnOnce(&mut Active)) -> Option<SessionState> {
        let mut guard = self.active.lock().unwrap();
        let active = guard.as_mut().filter(|a| a.generation == generation)?;
        f(active);
        Some(active.state.clone())
    }

    fn finish(&self, generation: u64) {
        let mut guard = self.active.lock().unwrap();
        if guard.as_ref().is_some_and(|a| a.generation == generation) {
            *guard = None;
        }
    }
}

pub fn launch(app: &AppHandle, game: &Game) -> Result<()> {
    let spawned = spawn(app, &game.launch).with_context(|| format!("avvio di {}", game.name))?;
    // A spawned store client (Galaxy, Steam…) is not the game: follow the
    // spawned process only for hand-added executables, or when there is no
    // install folder to watch instead.
    let track_child = matches!(game.store, Store::Manual | Store::Mock) || game.install_dir.is_none();
    let child_pid = spawned.filter(|_| track_child);

    let launcher = &app.state::<AppState>().launcher;
    // Starting a new game supersedes tracking of the previous one.
    let generation = launcher.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let state = SessionState { game_id: game.id.clone(), phase: Phase::Launching, started_at: now(), seconds: 0 };
    *launcher.active.lock().unwrap() = Some(Active { generation, state: state.clone(), pids: Vec::new() });
    let _ = app.emit("game://state", &state);

    let app = app.clone();
    let install_dir = game.install_dir.clone();
    std::thread::spawn(move || monitor(app, generation, install_dir, child_pid));
    Ok(())
}

fn spawn(app: &AppHandle, spec: &LaunchSpec) -> Result<Option<u32>> {
    match spec {
        LaunchSpec::Uri { uri } => {
            // `-silent` keeps Steam's main window from popping over everything.
            if let Some(steam_exe) = uri.starts_with("steam://").then(steam_exe).flatten() {
                Command::new(steam_exe).args(["-silent", uri.as_str()]).spawn()?;
                return Ok(None);
            }
            app.opener().open_url(uri, None::<&str>)?;
            Ok(None)
        }
        LaunchSpec::AppsFolder { aumid } => {
            Command::new("explorer.exe").arg(format!(r"shell:AppsFolder\{aumid}")).spawn()?;
            Ok(None)
        }
        LaunchSpec::Exe { path, args, cwd } => {
            let mut cmd = Command::new(path);
            cmd.args(args);
            if let Some(dir) = cwd.as_deref().or_else(|| path.parent()) {
                cmd.current_dir(dir);
            }
            let mut child = cmd.spawn()?;
            let pid = child.id();
            // Reap the child so it doesn't linger as a zombie on Unix.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(Some(pid))
        }
    }
}

fn monitor(app: AppHandle, generation: u64, install_dir: Option<PathBuf>, child_pid: Option<u32>) {
    let launcher = &app.state::<AppState>().launcher;
    let refresh = ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet);
    let mut sys = System::new();
    let started = Instant::now();
    let mut first_seen: Option<Instant> = None;
    let mut last_seen = Instant::now();
    let mut handed_off = false;

    loop {
        // A newer launch took over.
        if launcher.update(generation, |_| {}).is_none() {
            return;
        }
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
        let pids = game_pids(&sys, install_dir.as_deref(), child_pid);

        if !pids.is_empty() {
            last_seen = Instant::now();
            let was_launching = first_seen.is_none();
            let first = *first_seen.get_or_insert(last_seen);
            // Hand the screen to the game only once its window exists. GameMode
            // stays fullscreen behind it, so the desktop never shows through
            // while a launcher or anti-cheat process starts first.
            if !handed_off {
                handed_off = window::focus_process(&pids);
            }
            let state = launcher.update(generation, |a| {
                a.pids = pids;
                a.state.phase = Phase::Running;
                a.state.seconds = first.elapsed().as_secs();
            });
            if was_launching {
                if let Some(state) = state {
                    let _ = app.emit("game://state", &state);
                }
            }
        } else {
            match first_seen {
                None if started.elapsed() > LAUNCH_TIMEOUT => {
                    end(&app, generation, Phase::Failed, 0);
                    return;
                }
                Some(first) if last_seen.elapsed() > EXIT_GRACE => {
                    let seconds = last_seen.duration_since(first).as_secs();
                    end(&app, generation, Phase::Exited, seconds);
                    return;
                }
                _ => {}
            }
        }
        std::thread::sleep(POLL);
    }
}

fn game_pids(sys: &System, install_dir: Option<&Path>, child_pid: Option<u32>) -> Vec<u32> {
    let dir = install_dir.map(normalize).filter(|d| d.len() > 3);
    sys.processes()
        .iter()
        .filter(|(pid, process)| {
            Some(pid.as_u32()) == child_pid
                || dir.as_ref().is_some_and(|d| {
                    process.exe().is_some_and(|exe| normalize(exe).starts_with(&format!("{d}/")))
                })
        })
        .map(|(pid, _)| pid.as_u32())
        .collect()
}

/// Case- and separator-insensitive path string, as Windows compares paths.
fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

fn end(app: &AppHandle, generation: u64, phase: Phase, seconds: u64) {
    let state = app.state::<AppState>();
    let Some(session) = state.launcher.update(generation, |a| {
        a.state.phase = phase;
        a.state.seconds = seconds;
    }) else {
        return;
    };
    state.launcher.finish(generation);

    if phase == Phase::Exited && seconds > 0 {
        let record = Session { game_id: session.game_id.clone(), start: session.started_at, end: now(), seconds };
        match state.db.update(|d| {
            d.record_session(record);
            d.game(&session.game_id).cloned()
        }) {
            Ok(Some(game)) => {
                let _ = app.emit("library://game", &game);
            }
            Ok(None) => {}
            Err(err) => eprintln!("saving session failed: {err:#}"),
        }
    }
    let _ = app.emit("game://state", &session);
    window::bring_to_front(app);
}

#[cfg(windows)]
fn steam_exe() -> Option<PathBuf> {
    crate::library::steam::find_steam_root().map(|root| root.join("steam.exe")).filter(|p| p.is_file())
}

#[cfg(not(windows))]
fn steam_exe() -> Option<PathBuf> {
    None
}

/// Starts Steam in the background if it isn't running, so launching a Steam
/// game later neither waits for Steam nor shows its window.
pub fn preload_steam() {
    let Some(exe) = steam_exe() else { return };
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let running = sys.processes().values().any(|p| p.name().eq_ignore_ascii_case("steam.exe"));
    if !running {
        if let Err(err) = Command::new(exe).arg("-silent").spawn() {
            eprintln!("starting Steam failed: {err}");
        }
    }
}

/// Ends the running game's processes.
pub fn close_running(app: &AppHandle) -> bool {
    let pids = app.state::<AppState>().launcher.running_pids();
    if pids.is_empty() {
        return false;
    }
    let mut sys = System::new();
    let targets: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
    sys.refresh_processes(ProcessesToUpdate::Some(&targets), true);
    for pid in targets {
        if let Some(process) = sys.process(pid) {
            process.kill();
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_windows_paths() {
        assert_eq!(normalize(Path::new(r"C:\Games\Hades\")), "c:/games/hades");
    }
}
