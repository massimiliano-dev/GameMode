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
use crate::updates::GameUpdate;
use crate::{window, AppState};

const POLL: Duration = Duration::from_secs(2);
/// How long a store client may take before the game process shows up
/// (updates, sign-in, shader caches…).
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Game Pass games can sit in Gaming Services for minutes on first start.
const LAUNCH_TIMEOUT_XBOX: Duration = Duration::from_secs(10 * 60);
/// Processes running this long count as the game even without a window
/// GameMode can see (UWP games draw in a frame owned by another process).
const CONFIRM_WITHOUT_WINDOW: Duration = Duration::from_secs(60);
/// How many polls GameMode keeps pushing a new game window to the front.
const HANDOFF_TRIES: u32 = 15;
/// A store window must stay up this long before GameMode steps aside for
/// it (Steam's short "preparing to launch" box doesn't count).
const DIALOG_SETTLE: Duration = Duration::from_secs(6);
/// After a Game Pass update, how long to wait before starting the game again.
const RELAUNCH_AFTER_UPDATE: Duration = Duration::from_secs(15);
/// Store clients and helpers whose windows (sign-in, updates, errors, EULAs)
/// a launch may wait on.
const LAUNCHERS: &[&str] = &[
    "steam.exe",
    "steamwebhelper.exe",
    "xboxpcapp.exe",
    "gamelaunchhelper.exe",
    "gamingservicesui.exe",
    "epicgameslauncher.exe",
    "eadesktop.exe",
    "upc.exe",
    "ubisoftconnect.exe",
    "galaxyclient.exe",
    "battle.net.exe",
];
/// Package helpers that run before the game itself and aren't the game.
const NOT_THE_GAME: &[&str] = &["gamelaunchhelper.exe"];
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
    /// The game is being updated before it can start.
    pub update: Option<GameUpdate>,
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
    crate::assist::disable();
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
    let state =
        SessionState { game_id: game.id.clone(), phase: Phase::Launching, started_at: now(), seconds: 0, update: None };
    *launcher.active.lock().unwrap() = Some(Active { generation, state: state.clone(), pids: Vec::new() });
    let _ = app.emit("game://state", &state);

    let timeout = if game.store == Store::Xbox { LAUNCH_TIMEOUT_XBOX } else { LAUNCH_TIMEOUT };
    let target = Target {
        dir: game.install_dir.as_deref().map(normalize).filter(|d| d.len() > 3),
        // Xbox game ids are `xbox:<PackageFamilyName>`.
        family: (game.store == Store::Xbox).then(|| native_id(game).to_lowercase()),
        child_pid,
    };
    let app = app.clone();
    let game = game.clone();
    std::thread::spawn(move || monitor(app, generation, game, target, timeout));
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
        let helper = info.exe.as_deref().and_then(Path::file_name).is_some_and(|name| {
            NOT_THE_GAME.contains(&name.to_string_lossy().to_lowercase().as_str())
        });
        if helper {
            return false;
        }
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
            // Packaged games, like the Xbox app, don't show up without
            // Explorer: in console mode it starts in the background first.
            if crate::shell::status().active {
                crate::shell::ensure_explorer();
            }
            if Command::new("explorer.exe").arg(format!(r"shell:AppsFolder\{aumid}")).spawn().is_err() {
                activate_packaged(aumid)?;
            }
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

fn monitor(app: AppHandle, generation: u64, game: Game, target: Target, timeout: Duration) {
    let launcher = &app.state::<AppState>().launcher;
    let refresh = ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet);
    let mut sys = System::new();
    let mut probe = Probe::default();
    // The launch gives up after `timeout` without news; an update in
    // progress counts as news.
    let mut waiting_since = Instant::now();
    // Set once the game is really up: its window showed, or its processes
    // kept running for a while. Before that, store helpers (Game Pass's
    // launch helper, licence checks, updaters) come and go without ending
    // the launch.
    let mut running_since: Option<Instant> = None;
    let mut alive_since: Option<Instant> = None;
    let mut last_seen = Instant::now();
    // The game window last handed the screen, and attempts at it so far.
    let mut handed: Option<isize> = None;
    let mut tries = 0;
    // Since when the game's window is gone while its processes linger.
    let mut window_gone: Option<Instant> = None;
    let mut update: Option<GameUpdate> = None;
    let mut relaunch_at: Option<Instant> = None;
    let mut dialogs = Dialogs::new();

    loop {
        // A newer launch took over.
        if launcher.update(generation, |_| {}).is_none() {
            return;
        }
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
        probe.retain_alive(sys.processes());
        let pids = game_pids(&sys, &mut probe, &target);
        let window = if pids.is_empty() { None } else { window::game_window(&pids) };

        if running_since.is_none() {
            // Updates first: say what's going on, and keep waiting meanwhile.
            let now_update = crate::updates::for_game(&game);
            if now_update.as_ref().is_some_and(GameUpdate::active) {
                waiting_since = Instant::now();
            }
            if now_update != update {
                // Game Pass doesn't start the game after updating it: do it.
                if game.store == Store::Xbox && now_update.is_none() && update.as_ref().is_some_and(GameUpdate::active) {
                    relaunch_at = Some(Instant::now() + RELAUNCH_AFTER_UPDATE);
                }
                update = now_update;
                if let Some(state) = launcher.update(generation, |a| a.state.update = update.clone()) {
                    let _ = app.emit("game://state", &state);
                }
            }
            if relaunch_at.is_some_and(|at| Instant::now() >= at) {
                relaunch_at = None;
                if pids.is_empty() {
                    let _ = spawn(&app, &game.launch);
                }
            }
            if window.is_none() {
                dialogs.poll(&app);
            }
        }

        if !pids.is_empty() {
            last_seen = Instant::now();
            let alive = *alive_since.get_or_insert(last_seen);
            // Hand the screen to the game only once its window exists. GameMode
            // stays fullscreen behind it, so the desktop never shows through
            // while a launcher or anti-cheat process starts first. A new
            // window (splash → game) gets handed the screen again; the same
            // one doesn't, so coming back to GameMode with Guide sticks.
            if let Some(hwnd) = window {
                if handed != Some(hwnd) {
                    handed = Some(hwnd);
                    tries = 0;
                }
                if tries < HANDOFF_TRIES {
                    tries += 1;
                    window::force_foreground_raw(hwnd);
                    // Step aside so GameMode can't cover the game; if the
                    // foreground lock held, this also activates the game,
                    // the next window down.
                    #[cfg(windows)]
                    window::minimize(&app);
                    if window::is_hwnd_foreground(hwnd) {
                        tries = HANDOFF_TRIES;
                    }
                }
            }
            let was_launching = running_since.is_none();
            if was_launching && (window.is_some() || alive.elapsed() > CONFIRM_WITHOUT_WINDOW) {
                running_since = Some(last_seen);
                dialogs.done();
            }
            let state = launcher.update(generation, |a| {
                a.pids = pids;
                if let Some(since) = running_since {
                    a.state.phase = Phase::Running;
                    a.state.seconds = since.elapsed().as_secs();
                    a.state.update = None;
                }
            });
            if was_launching && running_since.is_some() {
                if let Some(state) = state {
                    let _ = app.emit("game://state", &state);
                }
            }
            // Quitting a game can leave background processes behind (crash
            // reporters, launchers, services): once its window is gone for
            // good, the game is over.
            if let Some(since) = running_since.filter(|_| handed.is_some()) {
                if window.is_some() {
                    window_gone = None;
                } else {
                    let gone = *window_gone.get_or_insert_with(Instant::now);
                    if gone.elapsed() > EXIT_GRACE {
                        end(&app, generation, Phase::Exited, gone.duration_since(since).as_secs());
                        return;
                    }
                }
            }
        } else {
            alive_since = None;
            match running_since {
                None if waiting_since.elapsed() > timeout => {
                    // Never detected: the game may well be running anyway, so
                    // don't pull the launcher over it.
                    dialogs.done();
                    end(&app, generation, Phase::Failed, 0);
                    return;
                }
                Some(since) if last_seen.elapsed() > EXIT_GRACE => {
                    let seconds = last_seen.duration_since(since).as_secs();
                    end(&app, generation, Phase::Exited, seconds);
                    return;
                }
                _ => {}
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Store windows that pop up while a game starts (sign-in, update, EULA,
/// "game requires an update"): GameMode would cover them, so it steps aside
/// for any that stays up, and comes back once they are gone.
struct Dialogs {
    /// Windows that were already there when the launch began.
    known: std::collections::HashSet<isize>,
    /// New windows and when they showed up.
    seen: std::collections::HashMap<isize, Instant>,
    /// The window GameMode stepped aside for.
    aside: Option<isize>,
    /// GameMode is out of the way for store windows.
    stepped_aside: bool,
}

impl Dialogs {
    fn new() -> Self {
        Dialogs { known: window::windows_of(LAUNCHERS).into_iter().collect(), seen: Default::default(), aside: None, stepped_aside: false }
    }

    fn poll(&mut self, app: &AppHandle) {
        let current = window::windows_of(LAUNCHERS);
        let now = Instant::now();
        self.seen.retain(|h, _| current.contains(h));
        for hwnd in current.iter().filter(|h| !self.known.contains(h)) {
            self.seen.entry(*hwnd).or_insert(now);
        }
        if self.aside.is_some_and(|h| !self.seen.contains_key(&h)) {
            self.aside = None;
        }
        if self.aside.is_none() {
            if let Some((&hwnd, _)) = self.seen.iter().find(|(_, at)| at.elapsed() >= DIALOG_SETTLE) {
                self.aside = Some(hwnd);
                self.stepped_aside = true;
                let keys = app.state::<AppState>().db.lock().settings.keyboard_assist;
                crate::assist::enable(keys);
                window::force_foreground_raw(hwnd);
                window::minimize(app);
            } else if self.stepped_aside && self.seen.is_empty() {
                // Every store window is gone and the game isn't up yet:
                // back to the launch screen.
                self.done();
                window::bring_to_front(app);
            }
        }
    }

    /// The game showed up (or the launch ended): stop handling store windows.
    fn done(&mut self) {
        if self.stepped_aside {
            crate::assist::disable();
        }
        self.stepped_aside = false;
        self.aside = None;
        self.seen.clear();
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
    crate::overlay::hide(app);
    // A failed launch leaves GameMode where it is, unless it stepped aside.
    if phase == Phase::Exited || window::is_minimized(app) {
        window::bring_to_front(app);
    }
}

#[cfg(windows)]
fn activate_packaged(aumid: &str) -> Result<u32> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::{ApplicationActivationManager, IApplicationActivationManager, AO_NONE};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let manager: IApplicationActivationManager =
            CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)?;
        Ok(manager.ActivateApplication(&HSTRING::from(aumid), PCWSTR::null(), AO_NONE)?)
    }
}

#[cfg(not(windows))]
fn activate_packaged(_aumid: &str) -> Result<u32> {
    anyhow::bail!("solo su Windows")
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
    let launcher = &app.state::<AppState>().launcher;
    let pids = launcher.running_pids();
    if pids.is_empty() {
        // Still starting: stop waiting for it.
        let generation = launcher.active.lock().unwrap().as_ref().map(|a| a.generation);
        if let Some(generation) = generation {
            end(app, generation, Phase::Exited, 0);
            return true;
        }
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
