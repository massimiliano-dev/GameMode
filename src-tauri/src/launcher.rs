//! Starting games and following them: store clients start the real game
//! process themselves, so a game is "running" while any process whose
//! executable lives inside its install folder is alive — or, for Xbox /
//! Game Pass titles, any process running with the game's package identity.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::procinfo::{ProcInfo, Probe};
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
    let settings = app.state::<AppState>().db.lock().settings.clone();
    if settings.performance_mode {
        crate::performance::begin(&settings.close_apps);
    }
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

    let target = Target {
        dir: game.install_dir.as_deref().map(normalize).filter(|d| d.len() > 3),
        // Xbox game ids are `xbox:<PackageFamilyName>`.
        family: (game.store == Store::Xbox).then(|| native_id(game).to_lowercase()),
        child_pid,
    };
    let app = app.clone();
    std::thread::spawn(move || monitor(app, generation, target));
    Ok(())
}

fn native_id(game: &Game) -> &str {
    game.id.split_once(':').map_or(game.id.as_str(), |(_, id)| id)
}

/// What identifies the processes of one game.
struct Target {
    /// Normalized install folder.
    dir: Option<String>,
    /// Lowercase package family name.
    family: Option<String>,
    child_pid: Option<u32>,
}

impl Target {
    fn matches(&self, pid: u32, info: &ProcInfo) -> bool {
        Some(pid) == self.child_pid
            || self.family.as_ref().is_some_and(|f| info.family.as_ref().is_some_and(|pf| pf.to_lowercase() == *f))
            || self.dir.as_ref().is_some_and(|d| {
                info.exe.as_deref().is_some_and(|exe| normalize(exe).starts_with(&format!("{d}/")))
            })
    }
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

fn monitor(app: AppHandle, generation: u64, target: Target) {
    let launcher = &app.state::<AppState>().launcher;
    let refresh = ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet);
    let mut sys = System::new();
    let mut probe = Probe::default();
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
        probe.retain_alive(sys.processes());
        let pids = game_pids(&sys, &mut probe, &target);

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
                    // Never detected: the game may well be running anyway, so
                    // don't pull the launcher over it.
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

fn game_pids(sys: &System, probe: &mut Probe, target: &Target) -> Vec<u32> {
    sys.processes()
        .iter()
        .filter(|(pid, process)| target.matches(pid.as_u32(), probe.info(**pid, process)))
        .map(|(pid, _)| pid.as_u32())
        .collect()
}

/// Case- and separator-insensitive path string, as Windows compares paths.
pub(crate) fn normalize(path: &Path) -> String {
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
        let record = Session {
            game_id: session.game_id.clone(),
            profile_id: String::new(),
            start: session.started_at,
            end: now(),
            seconds,
        };
        match state.db.update(|d| {
            d.record_session(record);
            d.view_game(&session.game_id)
        }) {
            Ok(Some(game)) => {
                let _ = app.emit("library://game", &game);
            }
            Ok(None) => {}
            Err(err) => eprintln!("saving session failed: {err:#}"),
        }
    }
    let _ = app.emit("game://state", &session);
    crate::performance::end();
    if phase == Phase::Exited {
        window::bring_to_front(app);
    }
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
    fn matches_by_folder_or_package_identity() {
        let target = Target {
            dir: Some(normalize(Path::new(r"C:\XboxGames\Forza\Content"))),
            family: Some("microsoft.624f8b84b80_8wekyb3d8bbwe".into()),
            child_pid: None,
        };
        let in_dir = ProcInfo { exe: Some(r"C:\XboxGames\Forza\Content\Forza.exe".into()), family: None };
        let packaged = ProcInfo {
            exe: Some(r"C:\Program Files\WindowsApps\X\Forza.exe".into()),
            family: Some("Microsoft.624F8B84B80_8wekyb3d8bbwe".into()),
        };
        let other = ProcInfo { exe: Some(r"C:\Windows\explorer.exe".into()), family: None };
        assert!(target.matches(1, &in_dir));
        assert!(target.matches(2, &packaged));
        assert!(!target.matches(3, &other));
        assert!(!target.matches(4, &ProcInfo::default()));
    }

    #[test]
    fn normalizes_windows_paths() {
        assert_eq!(normalize(Path::new(r"C:\Games\Hades\")), "c:/games/hades");
    }
}
