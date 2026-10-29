//! Starting games and following them: store clients start the real game
//! process themselves, so a game is "running" while any process whose
//! executable lives inside its install folder is alive — or, for Xbox /
//! Game Pass titles, any process running with the game's package identity.
//!
//! One game is in the foreground at a time; others can be frozen in place
//! (Quick Resume, see `freeze`) and kept in a short list of suspended
//! sessions, each still followed by its own monitor thread.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::live::LiveSession;
use crate::procinfo::{ProcInfo, Probe};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::model::{now, Game, LaunchSpec, Session, Settings, Store};
use crate::updates::GameUpdate;
use crate::{freeze, lifecycle, live, window, AppState};

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
/// How often the live-session file hears that the game is still there.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// How many games can be kept frozen at once.
pub const MAX_SUSPENDED: usize = 3;
/// Store clients and helpers whose windows (sign-in, updates, errors, EULAs)
/// a launch may wait on.
pub(crate) const LAUNCHERS: &[&str] = &[
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
/// Folders Steam (and games) keep first-run installers in: Visual C++,
/// DirectX, .NET, PhysX…
const PREREQ_FOLDERS: &[&str] = &["/steamworks shared/", "/_commonredist/", "/__installer/", "/redist/", "/redistributables/"];
/// Those installers by name, wherever they are.
const PREREQ_EXES: &[&str] = &[
    "vc_redist.x64.exe",
    "vc_redist.x86.exe",
    "vcredist_x64.exe",
    "vcredist_x86.exe",
    "dxsetup.exe",
    "dxwebsetup.exe",
    "ue4prereqsetup_x64.exe",
    "ueprereqsetup_x64.exe",
    "oalinst.exe",
    "physxsetup.exe",
    "easyanticheat_setup.exe",
    "easyanticheat_eos_setup.exe",
    "msiexec.exe",
];

/// A first-run installer a store runs before the game (Visual C++, DirectX,
/// .NET…): not the game, and a window GameMode must not cover, or the game
/// never starts.
fn is_prerequisite(exe: &Path) -> bool {
    let path = normalize(exe);
    let name = path.rsplit('/').next().unwrap_or_default();
    PREREQ_FOLDERS.iter().any(|f| path.contains(f))
        || PREREQ_EXES.contains(&name)
        || name.starts_with("dotnetfx")
        || name.starts_with("ndp")
        || name.starts_with("windowsdesktop-runtime")
}
/// Games often restart themselves (launcher → game, anti-cheat…).
const EXIT_GRACE: Duration = Duration::from_secs(10);
/// Steam's own windows (client, Big Picture, downloads page).
#[cfg_attr(not(windows), allow(dead_code))]
const STEAM_WINDOWS: &[&str] = &["steam.exe", "steamwebhelper.exe"];
/// After GameMode closes Big Picture, Steam's desktop window comes up for a
/// moment: it's sent away from the game for this long, also on the desktop.
const AFTER_BIG_PICTURE: Duration = Duration::from_secs(30);
/// How long an exit path waits for a state change under way.
const TRANSITION_WAIT: Duration = Duration::from_secs(3);

#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Launching,
    Running,
    /// Frozen in place (Quick Resume): alive, off screen, not counting play time.
    Suspended,
    Exited,
    Failed,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionState {
    pub game_id: String,
    /// The game's name, so a frozen game stays visible even once the
    /// library no longer lists it.
    pub game_name: String,
    pub phase: Phase,
    pub started_at: i64,
    pub seconds: u64,
    /// The game is being updated before it can start.
    pub update: Option<GameUpdate>,
    /// Unix seconds since when the game is frozen.
    pub suspended_at: Option<i64>,
    /// An anti-cheat lives in the install folder: freezing an online game
    /// gets the player kicked, so the UI asks first.
    pub anti_cheat: bool,
    /// What the launch is waiting on before the game starts (`steam_direct::STAGE_*`).
    pub stage: Option<String>,
}

struct Active {
    generation: u64,
    state: SessionState,
    pids: Vec<u32>,
    /// `(pid, start time)` of the same processes, for the live-session file.
    live_pids: Vec<(u32, u64)>,
    /// Unix seconds since the game was first seen running.
    running_since: Option<i64>,
    /// Seconds spent frozen so far: not play time.
    paused_secs: u64,
    /// The processes frozen by Quick Resume (the game and its children).
    frozen: Vec<u32>,
}

impl Active {
    /// Play time so far, pauses excluded.
    fn played(&self, at: i64) -> u64 {
        let since = self.running_since.map_or(0, |s| (at - s).max(0) as u64);
        let paused = self.paused_secs + self.state.suspended_at.map_or(0, |s| (at - s).max(0) as u64);
        since.saturating_sub(paused)
    }

    /// What the live-session file keeps about this session, once it runs.
    fn live_record(&self, at: i64) -> Option<LiveSession> {
        Some(LiveSession {
            game_id: self.state.game_id.clone(),
            started_at: self.running_since?,
            last_seen: at,
            pids: self.live_pids.clone(),
            suspended: self.state.phase == Phase::Suspended,
            paused_secs: self.paused_secs + self.state.suspended_at.map_or(0, |s| (at - s).max(0) as u64),
        })
    }
}

#[derive(Default)]
pub struct Launcher {
    sessions: Mutex<Vec<Active>>,
    generation: AtomicU64,
    /// Held while a game is frozen, thawed or closed: suspending the same
    /// game twice would need two thaws.
    transition: Mutex<()>,
}

impl Launcher {
    /// Claims the right to change a session's state, or says that another
    /// change is still under way.
    fn transition(&self) -> Result<MutexGuard<'_, ()>> {
        match self.transition.try_lock() {
            Ok(guard) => Ok(guard),
            Err(TryLockError::Poisoned(e)) => Ok(e.into_inner()),
            Err(TryLockError::WouldBlock) => anyhow::bail!("Un attimo: un altro gioco sta cambiando stato"),
        }
    }

    /// Like `transition`, waiting up to `timeout` for any change under way
    /// to finish. `None` past that: the exit paths run on the main thread,
    /// and a change waiting on the main thread must not hang them.
    fn transition_within(&self, timeout: Duration) -> Option<MutexGuard<'_, ()>> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.transition.try_lock() {
                Ok(guard) => return Some(guard),
                Err(TryLockError::Poisoned(e)) => return Some(e.into_inner()),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                Err(TryLockError::WouldBlock) => return None,
            }
        }
    }

    /// Ids and processes of every game GameMode follows, frozen ones included.
    fn open_games(&self) -> (Vec<String>, Vec<u32>) {
        let sessions = self.sessions.lock().unwrap();
        let ids = sessions.iter().map(|a| a.state.game_id.clone()).collect();
        let pids = sessions.iter().flat_map(|a| a.pids.iter().copied()).collect();
        (ids, pids)
    }

    /// Every game GameMode follows with its processes, frozen ones included.
    pub fn followed(&self) -> Vec<(SessionState, Vec<u32>)> {
        self.sessions.lock().unwrap().iter().map(|a| (a.state.clone(), a.pids.clone())).collect()
    }

    /// The game in the foreground (starting or running), if any.
    pub fn current(&self) -> Option<SessionState> {
        self.sessions.lock().unwrap().iter().find(|a| a.state.phase != Phase::Suspended).map(|a| a.state.clone())
    }

    /// The games frozen by Quick Resume.
    pub fn suspended(&self) -> Vec<SessionState> {
        self.sessions.lock().unwrap().iter().filter(|a| a.state.phase == Phase::Suspended).map(|a| a.state.clone()).collect()
    }

    pub fn running_pids(&self) -> Vec<u32> {
        self.sessions.lock().unwrap().iter().find(|a| a.state.phase != Phase::Suspended).map(|a| a.pids.clone()).unwrap_or_default()
    }

    fn phase(&self, generation: u64) -> Option<Phase> {
        self.sessions.lock().unwrap().iter().find(|a| a.generation == generation).map(|a| a.state.phase)
    }

    fn generation_of(&self, game_id: &str) -> Option<u64> {
        self.sessions.lock().unwrap().iter().find(|a| a.state.game_id == game_id).map(|a| a.generation)
    }

    fn with<T>(&self, generation: u64, f: impl FnOnce(&mut Active) -> T) -> Option<T> {
        let mut guard = self.sessions.lock().unwrap();
        let active = guard.iter_mut().find(|a| a.generation == generation)?;
        Some(f(active))
    }

    fn update(&self, generation: u64, f: impl FnOnce(&mut Active)) -> Option<SessionState> {
        self.with(generation, |a| {
            f(a);
            a.state.clone()
        })
    }

    /// Adds a foreground session; it supersedes the previous foreground
    /// one (its monitor stops), never the suspended ones.
    fn insert(&self, active: Active) {
        let mut guard = self.sessions.lock().unwrap();
        guard.retain(|a| a.state.phase == Phase::Suspended);
        guard.push(active);
    }

    fn finish(&self, generation: u64) {
        self.sessions.lock().unwrap().retain(|a| a.generation != generation);
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.lock().unwrap().is_empty()
    }

    /// The session in the foreground (starting or running), if any.
    fn foreground_generation(&self) -> Option<u64> {
        self.sessions.lock().unwrap().iter().find(|a| a.state.phase != Phase::Suspended).map(|a| a.generation)
    }

    /// Processes frozen by sessions other than `generation`: never part of
    /// another session (a launcher app whose folder holds a frozen game).
    fn frozen_elsewhere(&self, generation: u64) -> Vec<u32> {
        self.sessions.lock().unwrap().iter().filter(|a| a.generation != generation).flat_map(|a| a.frozen.iter().copied()).collect()
    }

    /// Takes `secs` (the PC was asleep) out of the play time of every game
    /// that was running; frozen ones don't count time anyway. Returns the
    /// sessions touched, with their play time refreshed.
    fn add_pause(&self, secs: u64, at: i64) -> Vec<(u64, SessionState)> {
        let mut guard = self.sessions.lock().unwrap();
        guard
            .iter_mut()
            .filter(|a| a.state.phase != Phase::Suspended && a.running_since.is_some())
            .map(|a| {
                a.paused_secs += secs;
                a.state.seconds = a.played(at);
                (a.generation, a.state.clone())
            })
            .collect()
    }
}

/// Back from sleep after `slept` seconds: that wasn't play time.
pub fn after_sleep(app: &AppHandle, slept: u64) {
    if slept == 0 {
        return;
    }
    let launcher = &app.state::<AppState>().launcher;
    for (generation, state) in launcher.add_pause(slept, now()) {
        heartbeat(launcher, generation);
        let _ = app.emit("game://state", &state);
    }
}

pub fn launch(app: &AppHandle, game: &Game) -> Result<()> {
    // A frozen game is brought back, not started twice.
    let launcher = &app.state::<AppState>().launcher;
    if launcher.generation_of(&game.id).is_some_and(|g| launcher.phase(g) == Some(Phase::Suspended)) {
        return resume_suspended(app, &game.id);
    }
    crate::assist::disable();
    // An app over a running game keeps the game frozen (Quick Resume), as a
    // console does; a game over a game still replaces it.
    let parked = if game.store == Store::App { park_running_game(app) } else { None };
    let settings = app.state::<AppState>().db.lock().settings.clone();
    // Apps don't get the performance plan, nor have their own windows closed.
    if settings.performance_mode && game.store != Store::App {
        crate::performance::begin(&settings.close_apps);
    }
    // Steam games go to a silent, signed-in Steam with `-applaunch`.
    if let (Some(appid), Some(exe)) = (crate::steam_direct::appid_of(game), steam_exe()) {
        return launch_steam(app, game, &exe, appid);
    }
    let spawned = match spawn(app, &launch_spec(&settings, game)) {
        Ok(spawned) => spawned,
        Err(err) => {
            // The app didn't start: the game it made room for comes back.
            if let Some(generation) = parked {
                unpark(app, generation);
            }
            return Err(err.context(format!("avvio di {}", game.name)));
        }
    };
    // A spawned store client (Galaxy, Steam…) is not the game: follow the
    // spawned process only for hand-added executables, or when there is no
    // install folder to watch instead.
    let track_child = matches!(game.store, Store::Manual | Store::Mock | Store::App) || game.install_dir.is_none();
    let child_pid = spawned.filter(|_| track_child);
    retire_foreground(app);
    let _ = track(app, game, child_pid, None, 0, None);
    Ok(())
}

/// How to start `game`: GOG games skip Galaxy, unless the user wants it, when
/// their folder says how they start.
fn launch_spec(settings: &Settings, game: &Game) -> LaunchSpec {
    if game.store == Store::Gog && settings.gog_direct {
        if let Some(spec) = game.install_dir.as_deref().and_then(|dir| crate::library::gog::direct_launch(dir, native_id(game))) {
            return spec;
        }
    }
    game.launch.clone()
}

/// Starts a Steam game: the launch screen shows while Steam gets ready
/// (started silently, signed in), then the game goes to it with
/// `-applaunch`, so neither the library nor the protocol handler shows up.
/// Cancelling the launch meanwhile starts nothing.
/// No process of the game this long after `-applaunch`: Steam dropped it
/// (still loading, or a Steam that ignores it), so GameMode tries again,
/// first with Steam's own link, the way its shortcuts start games.
const APPLAUNCH_RETRY: Duration = Duration::from_secs(15);
const APPLAUNCH_RETRIES: u32 = 2;

fn launch_steam(app: &AppHandle, game: &Game, exe: &Path, appid: u32) -> Result<()> {
    retire_foreground(app);
    let generation = track(app, game, None, None, 0, None);
    let launcher = &app.state::<AppState>().launcher;
    let wanted = || launcher.phase(generation) == Some(Phase::Launching);
    let show = |stage: Option<&str>| {
        if let Some(state) = launcher.update(generation, |a| a.state.stage = stage.map(String::from)) {
            let _ = app.emit("game://state", &state);
        }
    };
    if !crate::steam_direct::ensure_ready(exe, wanted, show) {
        return Ok(());
    }
    if let Err(err) = crate::steam_direct::applaunch(exe, appid) {
        log::warn!("steam -applaunch failed, using the link: {err}");
        if let Err(err) = spawn(app, &game.launch) {
            end(app, generation, Phase::Failed, 0);
            return Err(err.context(format!("avvio di {}", game.name)));
        }
        return Ok(());
    }
    retry_applaunch(app, generation, game, exe, appid);
    Ok(())
}

/// Sends the game to Steam again while nothing of it has started. Only when
/// that's certain: GameMode's launch screen still has the screen (the game,
/// or a Steam dialog, would have taken it), Steam doesn't report the game
/// running or updating, and none of its processes showed up. A second
/// launch of a game that did start makes Steam put a dialog over it.
fn retry_applaunch(app: &AppHandle, generation: u64, game: &Game, exe: &Path, appid: u32) {
    let (app, game, exe) = (app.clone(), game.clone(), exe.to_path_buf());
    std::thread::spawn(move || {
        let launcher = &app.state::<AppState>().launcher;
        for attempt in 1..=APPLAUNCH_RETRIES {
            std::thread::sleep(APPLAUNCH_RETRY * attempt);
            let waiting = launcher.with(generation, |a| a.state.phase == Phase::Launching && a.pids.is_empty() && a.state.update.is_none());
            if waiting != Some(true) {
                return;
            }
            let steam_idle = crate::steam_direct::app_busy(appid) == Some(false);
            if !steam_idle || !window::is_foreground(&app) || crate::overlay::is_open() {
                log::info!("{} may be starting (Steam busy: {}, GameMode in front: {}): not sending it again", game.name, !steam_idle, window::is_foreground(&app));
                return;
            }
            log::info!("{} hasn't started after -applaunch: trying again ({attempt})", game.name);
            let sent = if attempt == 1 {
                spawn(&app, &game.launch).map(|_| ())
            } else {
                crate::steam_direct::applaunch(&exe, appid).map_err(anyhow::Error::from)
            };
            if let Err(err) = sent {
                log::warn!("starting {} again failed: {err:#}", game.name);
                return;
            }
        }
    });
}

/// Closes the foreground session a launch replaces without freezing it:
/// its play time is recorded and the live-session file forgets it, while
/// its processes are left alone (a game left running under an app, an app
/// a game starts over).
fn retire_foreground(app: &AppHandle) {
    let launcher = &app.state::<AppState>().launcher;
    let Some(generation) = launcher.foreground_generation() else { return };
    let at = now();
    let Some(session) = launcher.update(generation, |a| {
        a.state.seconds = a.played(at);
        a.state.phase = Phase::Exited;
    }) else {
        return;
    };
    launcher.finish(generation);
    live::update(|l| l.remove(&session.game_id));
    if session.seconds > 0 {
        record(app, &session.game_id, session.started_at, at, session.seconds);
    }
    let _ = app.emit("game://state", &session);
}

/// Follows a game: creates its session and starts the monitor. With
/// `running_since` the game is already up (reattached after a restart) and
/// its play time counts from there; with `frozen` it is up but suspended,
/// and those are its frozen processes. Returns the session's generation.
fn track(app: &AppHandle, game: &Game, child_pid: Option<u32>, running_since: Option<i64>, paused_secs: u64, frozen: Option<Vec<u32>>) -> u64 {
    let launcher = &app.state::<AppState>().launcher;
    let generation = launcher.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let phase = match (running_since, &frozen) {
        (Some(_), Some(_)) => Phase::Suspended,
        (Some(_), None) => Phase::Running,
        (None, _) => Phase::Launching,
    };
    let state = SessionState {
        game_id: game.id.clone(),
        game_name: game.name.clone(),
        phase,
        started_at: running_since.unwrap_or_else(now),
        seconds: 0,
        update: None,
        suspended_at: (phase == Phase::Suspended).then(now),
        anti_cheat: game.install_dir.as_deref().is_some_and(freeze::anti_cheat_in),
        stage: None,
    };
    launcher.insert(Active {
        generation,
        state: state.clone(),
        pids: Vec::new(),
        live_pids: Vec::new(),
        running_since,
        paused_secs,
        frozen: frozen.unwrap_or_default(),
    });
    let _ = app.emit("game://state", &state);

    let timeout = if game.store == Store::Xbox { LAUNCH_TIMEOUT_XBOX } else { LAUNCH_TIMEOUT };
    let target = Target::of(game, child_pid);
    let app = app.clone();
    let game = game.clone();
    std::thread::spawn(move || monitor(app, generation, game, target, timeout, running_since.is_some()));
    generation
}

/// After a restart (crash, update): picks up the games the live-session
/// file remembers. A game still alive is followed again, frozen ones as
/// suspended (the user resumes them from Home); a dead one gets its play
/// time recorded. One game at a time has the foreground: a second running
/// one (the watchdog thaws frozen games when GameMode dies) is frozen again.
pub fn reattach(app: &AppHandle) {
    let mut remembered = live::snapshot().sessions;
    if remembered.is_empty() {
        // A crash mid-game may have left the performance power plan on.
        crate::performance::end();
        return;
    }
    // Frozen games first: the foreground goes to the first running one.
    remembered.sort_by_key(|s| !s.suspended);
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet));
    let mut probe = Probe::default();
    let state = app.state::<AppState>();
    let at = now();
    let mut foreground_taken = false;
    for s in remembered {
        let game = state.db.lock().game(&s.game_id).cloned();
        let same_pids = s.alive(|pid| sys.process(Pid::from_u32(pid)).map(|p| p.start_time()));
        let found = game.as_ref().map(|g| game_pids(&sys, &mut probe, &Target::of(g, None))).unwrap_or_default();
        match game {
            Some(game) if same_pids || !found.is_empty() => {
                let remembered_pids: Vec<u32> = s.pids.iter().map(|(pid, _)| *pid).collect();
                let pids = if found.is_empty() { remembered_pids } else { found };
                let frozen = if s.suspended {
                    Some(freeze::family(&pids))
                } else if foreground_taken {
                    // Running, but another game already has the screen.
                    let family = freeze::family(&pids);
                    match freeze::suspend(&family, window::game_window(&pids)) {
                        Ok(()) => Some(family),
                        Err(err) => {
                            log::warn!("freezing {} again failed: {err:#}", game.name);
                            live::update(|l| l.remove(&s.game_id));
                            if s.played() > 0 {
                                record(app, &s.game_id, s.started_at, s.last_seen, s.played());
                            }
                            continue;
                        }
                    }
                } else {
                    foreground_taken = true;
                    None
                };
                let _ = track(app, &game, None, Some(s.started_at), s.paused_until(at), frozen);
            }
            _ => {
                live::update(|l| l.remove(&s.game_id));
                if s.played() > 0 {
                    record(app, &s.game_id, s.started_at, s.last_seen, s.played());
                }
            }
        }
    }
    if state.launcher.is_empty() {
        crate::performance::end();
    }
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
    fn of(game: &Game, child_pid: Option<u32>) -> Self {
        Target {
            dir: game.install_dir.as_deref().map(normalize).filter(|d| d.len() > 3),
            // Xbox game ids are `xbox:<PackageFamilyName>`; a packaged app's
            // AUMID is `<PackageFamilyName>!<AppId>`.
            family: match (game.store, &game.launch) {
                (Store::Xbox, _) => Some(native_id(game).to_lowercase()),
                (Store::App, LaunchSpec::AppsFolder { aumid }) => aumid.split('!').next().map(str::to_lowercase),
                _ => None,
            },
            child_pid,
        }
    }

    fn matches(&self, pid: u32, info: &ProcInfo) -> bool {
        let helper = info.exe.as_deref().is_some_and(|exe| {
            is_prerequisite(exe)
                || exe.file_name().is_some_and(|name| NOT_THE_GAME.contains(&name.to_string_lossy().to_lowercase().as_str()))
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
                crate::webview2::scrub(Command::new(steam_exe).args(["-silent", uri.as_str()])).spawn()?;
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
            if crate::webview2::scrub(Command::new("explorer.exe").arg(format!(r"shell:AppsFolder\{aumid}"))).spawn().is_err() {
                activate_packaged(aumid)?;
            }
            Ok(None)
        }
        LaunchSpec::Exe { path, args, cwd } => {
            // Epic through legendary: it signs in, and may update first.
            if crate::legendary::is_exe(path) {
                crate::legendary::spawn_launch(app, args)?;
                return Ok(None);
            }
            let mut cmd = Command::new(path);
            cmd.args(args);
            if let Some(dir) = cwd.as_deref().or_else(|| path.parent()) {
                cmd.current_dir(dir);
            }
            let mut child = crate::webview2::scrub(&mut cmd).spawn()?;
            let pid = child.id();
            // Reap the child so it doesn't linger as a zombie on Unix.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(Some(pid))
        }
    }
}

fn monitor(app: AppHandle, generation: u64, game: Game, target: Target, timeout: Duration, reattached: bool) {
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
    let mut running_since: Option<Instant> = reattached.then(Instant::now);
    let mut alive_since: Option<Instant> = None;
    let mut last_seen = Instant::now();
    let mut last_heartbeat = Instant::now();
    // The game window last handed the screen, and attempts at it so far.
    let mut handed: Option<isize> = None;
    let mut tries = 0;
    // A reattached game already has the screen: its window is adopted, not handed over again.
    let mut adopt_window = reattached;
    // Since when the game's window is gone while its processes linger.
    let mut window_gone: Option<Instant> = None;
    let mut update: Option<GameUpdate> = None;
    let mut relaunch_at: Option<Instant> = None;
    let mut dialogs = Dialogs::new();
    // When GameMode closed the Big Picture it had opened for this launch.
    let mut big_picture_closed: Option<Instant> = None;
    // Windows doesn't count controller input: this thread keeps the PC up
    // while the game is starting or running (not while frozen), and not
    // after an unattended wake until the user shows up, so Windows can put
    // a PC woken by a timer back to sleep.
    let mut awake = lifecycle::Awake::default();
    let mut wakes = lifecycle::wakes();

    loop {
        // A newer launch took over, or the session was closed.
        let Some(phase) = launcher.phase(generation) else { return };
        awake.set(phase != Phase::Suspended && lifecycle::user_present());
        // Back from sleep: clocks jumped, the game (if still there) gets a
        // fresh grace period instead of being written off at once.
        let woke = lifecycle::wakes();
        if woke != wakes {
            wakes = woke;
            last_seen = Instant::now();
            waiting_since = Instant::now();
            window_gone = None;
        }
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
        probe.retain_alive(sys.processes());
        let mut pids = game_pids(&sys, &mut probe, &target);
        if phase != Phase::Suspended {
            let frozen = launcher.frozen_elsewhere(generation);
            pids.retain(|pid| !frozen.contains(pid));
        }

        if phase == Phase::Suspended {
            // Frozen: no window lookups (a frozen window answers nothing),
            // no play time; only whether its processes are still there.
            // Its window is minimized, not gone.
            window_gone = None;
            if pids.is_empty() {
                if last_seen.elapsed() > EXIT_GRACE {
                    end(&app, generation, Phase::Exited, 0);
                    return;
                }
            } else {
                last_seen = Instant::now();
                let live_pids = pids_with_start(&sys, &pids);
                launcher.with(generation, |a| {
                    a.pids = pids;
                    a.live_pids = live_pids;
                });
                if last_heartbeat.elapsed() >= HEARTBEAT {
                    last_heartbeat = Instant::now();
                    heartbeat(launcher, generation);
                }
            }
            std::thread::sleep(POLL);
            continue;
        }

        let window = if pids.is_empty() { None } else { window::game_window(&pids) };
        if adopt_window && !pids.is_empty() {
            adopt_window = false;
            handed = window;
            tries = HANDOFF_TRIES;
        }
        // The game's window keeps it alive even when its processes can't be
        // read any more (anti-cheat, a launcher that started it).
        let window_alive = running_since.is_some() && handed.is_some_and(window::window_alive);

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
                // Steam's downloads, opened in Big Picture while the game
                // was updating, would stay over it: the game has started.
                if crate::storefront::close_our_big_picture(&app) {
                    big_picture_closed = Some(Instant::now());
                }
            }
            // Steam took the screen from the running game (its window
            // coming back after Big Picture, the downloads page): it goes
            // down and the game gets the screen again.
            let started = running_since.filter(|_| tries >= HANDOFF_TRIES);
            if let (Some(hwnd), Some(started)) = (window, started) {
                let console = crate::shell::status().active && !crate::storefront::handed_off_after(started);
                let just_closed = big_picture_closed.is_some_and(|at| at.elapsed() < AFTER_BIG_PICTURE);
                if (console || just_closed) && steam_over_game() {
                    log::info!("Steam covered {}: giving the game the screen back", game.name);
                    #[cfg(windows)]
                    window::minimize_foreground_of(STEAM_WINDOWS);
                    window::force_foreground_raw(hwnd);
                    // Keep at it for a few polls, as at the start.
                    tries = HANDOFF_TRIES.saturating_sub(3);
                }
            }
            let at = now();
            let live_pids = pids_with_start(&sys, &pids);
            let state = launcher.with(generation, |a| {
                // Frozen during this poll (Quick Resume): leave it as it is,
                // the next poll follows it as suspended.
                if a.state.phase == Phase::Suspended {
                    return None;
                }
                a.pids = pids;
                a.live_pids = live_pids;
                if running_since.is_some() {
                    a.state.phase = Phase::Running;
                    a.running_since.get_or_insert(at);
                    a.state.seconds = a.played(at);
                    a.state.update = None;
                    a.state.stage = None;
                }
                Some(a.state.clone())
            });
            let Some(Some(state)) = state else {
                std::thread::sleep(POLL);
                continue;
            };
            if was_launching && running_since.is_some() {
                let _ = app.emit("game://state", &state);
                // From now on the session survives a restart of GameMode.
                last_heartbeat = Instant::now();
                heartbeat(launcher, generation);
            } else if running_since.is_some() && last_heartbeat.elapsed() >= HEARTBEAT {
                last_heartbeat = Instant::now();
                heartbeat(launcher, generation);
            }
            // Quitting a game can leave background processes behind (crash
            // reporters, launchers, services): once its window is gone for
            // good, the game is over.
            if running_since.is_some() && handed.is_some() {
                if window.is_some() || window_alive {
                    window_gone = None;
                } else {
                    let gone = *window_gone.get_or_insert_with(Instant::now);
                    if gone.elapsed() > EXIT_GRACE {
                        end(&app, generation, Phase::Exited, gone.elapsed().as_secs());
                        return;
                    }
                }
            }
        } else if window_alive {
            last_seen = Instant::now();
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
                Some(_) if last_seen.elapsed() > EXIT_GRACE => {
                    end(&app, generation, Phase::Exited, last_seen.elapsed().as_secs());
                    return;
                }
                _ => {}
            }
        }
        std::thread::sleep(POLL);
    }
}

/// Writes the session to the live-session file.
fn heartbeat(launcher: &Launcher, generation: u64) {
    if let Some(Some(record)) = launcher.with(generation, |a| a.live_record(now())) {
        live::update(|l| l.upsert(record));
    }
}

/// A window of Steam's has the foreground, and nobody asked for it: not the
/// in-game overlay (GameMode's, open), not a window picked in the task
/// switcher.
fn steam_over_game() -> bool {
    if crate::overlay::is_open() || crate::switcher::picked(window::foreground_raw()) {
        return false;
    }
    #[cfg(windows)]
    {
        window::foreground_process_name().is_some_and(|n| STEAM_WINDOWS.contains(&n.as_str()))
    }
    #[cfg(not(windows))]
    false
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
        Dialogs { known: Self::windows().into_iter().collect(), seen: Default::default(), aside: None, stepped_aside: false }
    }

    /// Store clients' windows, and first-run installers' (a fresh install
    /// runs Visual C++, DirectX… before the game, some asking for a click).
    fn windows() -> Vec<isize> {
        let mut all = window::windows_of(LAUNCHERS);
        all.extend(window::windows_where(is_prerequisite));
        all
    }

    fn poll(&mut self, app: &AppHandle) {
        let current = Self::windows();
        let now = Instant::now();
        self.seen.retain(|h, _| current.contains(h));
        for hwnd in current.iter().filter(|h| !self.known.contains(h)) {
            self.seen.entry(*hwnd).or_insert(now);
        }
        if self.aside.is_some_and(|h| !self.seen.contains_key(&h)) {
            self.aside = None;
        }
        // Only when it's GameMode covering them; and a full-screen app in
        // front is most likely the game, started in a way GameMode can't
        // follow: leave that alone.
        let covering = window::is_foreground(app) || self.stepped_aside;
        if self.aside.is_none() && covering && !window::fullscreen_app_in_front(app) {
            if let Some((&hwnd, _)) = self.seen.iter().find(|(_, at)| at.elapsed() >= DIALOG_SETTLE) {
                log::info!("stepping aside for a store or installer window during the launch");
                self.aside = Some(hwnd);
                self.stepped_aside = true;
                let keys = app.state::<AppState>().db.lock().settings.keyboard_assist;
                crate::assist::enable(keys);
                window::move_to_gamemode_screen(app, hwnd, false);
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

fn pids_with_start(sys: &System, pids: &[u32]) -> Vec<(u32, u64)> {
    pids.iter().filter_map(|&pid| sys.process(Pid::from_u32(pid)).map(|p| (pid, p.start_time()))).collect()
}

/// Case- and separator-insensitive path string, as Windows compares paths.
pub(crate) fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// Adds a finished session to the library and tells the frontend.
fn record(app: &AppHandle, game_id: &str, start: i64, end: i64, seconds: u64) {
    let state = app.state::<AppState>();
    let record = Session { game_id: game_id.to_string(), profile_id: String::new(), start, end, seconds };
    match state.db.update(|d| {
        d.record_session(record);
        d.view_game(game_id)
    }) {
        Ok(Some(game)) => {
            let _ = app.emit("library://game", &game);
        }
        Ok(None) => {}
        Err(err) => log::warn!("saving session failed: {err:#}"),
    }
}

/// Ends a session. `grace` is how long ago the game really went away.
fn end(app: &AppHandle, generation: u64, phase: Phase, grace: u64) {
    let state = app.state::<AppState>();
    let at = now();
    let Some((session, foreground, frozen)) = state.launcher.with(generation, |a| {
        let foreground = a.state.phase != Phase::Suspended;
        a.state.phase = phase;
        a.state.seconds = if phase == Phase::Exited { a.played(at).saturating_sub(grace) } else { 0 };
        a.state.suspended_at = None;
        (a.state.clone(), foreground, std::mem::take(&mut a.frozen))
    }) else {
        return;
    };
    state.launcher.finish(generation);
    live::update(|l| l.remove(&session.game_id));
    // Windows remembers an app's mute: a game that died frozen would start
    // silent next time.
    if !frozen.is_empty() {
        freeze::unmute(&frozen);
    }

    if phase == Phase::Exited && session.seconds > 0 {
        record(app, &session.game_id, session.started_at, at, session.seconds);
    }
    let _ = app.emit("game://state", &session);
    if state.launcher.is_empty() {
        crate::performance::end();
    }
    // A frozen game that died in the background changes nothing on screen.
    if !foreground {
        return;
    }
    crate::overlay::hide(app);
    crate::wand::forget();
    // A failed launch leaves GameMode where it is, unless it stepped aside;
    // and it never jumps over a full-screen app (a game it lost track of).
    if (phase == Phase::Exited || window::is_minimized(app)) && !window::fullscreen_app_in_front(app) {
        window::bring_to_front(app);
    }
}

/// Quick Resume: freezes the foreground game and brings GameMode back.
pub fn suspend_current(app: &AppHandle) -> Result<()> {
    // The overlay's own freeze goes first, so Quick Resume's mute sticks.
    crate::overlay_pause::thaw_now();
    let launcher = &app.state::<AppState>().launcher;
    let _transition = launcher.transition()?;
    let session = launcher.current().context("Nessun gioco in corso")?;
    if session.phase != Phase::Running {
        anyhow::bail!("Aspetta che il gioco sia partito");
    }
    if launcher.suspended().len() >= MAX_SUSPENDED {
        anyhow::bail!("Puoi tenere in pausa al massimo {MAX_SUSPENDED} giochi: chiudine uno");
    }
    let generation = launcher.generation_of(&session.game_id).context("Nessun gioco in corso")?;
    let pids = launcher.running_pids();
    let family = freeze::family(&pids);
    if family.is_empty() {
        anyhow::bail!("Questo gioco non si può sospendere: processi non trovati");
    }
    // Wand may hold the game frozen on its own count: let it go first.
    crate::wand::forget();
    let hwnd = window::game_window(&pids);
    freeze::suspend(&family, hwnd).map_err(|e| anyhow::anyhow!("Questo gioco non si può sospendere: {e:#}"))?;
    let at = now();
    let Some(state) = launcher.update(generation, |a| {
        a.state.seconds = a.played(at);
        a.state.phase = Phase::Suspended;
        a.state.suspended_at = Some(at);
        a.frozen = family.clone();
    }) else {
        // The game ended meanwhile: don't leave its processes frozen.
        let _ = freeze::resume(&family);
        anyhow::bail!("Il gioco si è chiuso");
    };
    heartbeat(launcher, generation);
    let _ = app.emit("game://state", &state);
    crate::overlay::hide(app);
    window::bring_to_front(app);
    Ok(())
}

/// Thaws a frozen game and hands it the screen. A game in the foreground
/// is frozen first, so one game at a time has the screen.
pub fn resume_suspended(app: &AppHandle, game_id: &str) -> Result<()> {
    let launcher = &app.state::<AppState>().launcher;
    let _transition = launcher.transition()?;
    let generation = launcher.generation_of(game_id).context("Questo gioco non è in pausa")?;
    if launcher.phase(generation) != Some(Phase::Suspended) {
        anyhow::bail!("Questo gioco non è in pausa");
    }
    if let Some(front) = launcher.current() {
        if front.phase == Phase::Launching {
            anyhow::bail!("Aspetta che il gioco in avvio sia partito");
        }
        if front.game_id.starts_with("app:") {
            // An app stays open as it is (a call, music): it just stops
            // being followed, as when a game starts over it.
            retire_foreground(app);
        } else {
            // The list may be full right now: swapping keeps its size.
            swap_out(app)?;
        }
    }
    thaw(app, generation)?;
    let pids = launcher.running_pids();
    window::resume_game(app, &pids);
    Ok(())
}

/// Freezes the running game before an app takes the screen, when that is
/// safe (no anti-cheat, room in the suspended list, the game is really up);
/// otherwise the app simply takes over, as a new game would. Returns the
/// frozen session.
fn park_running_game(app: &AppHandle) -> Option<u64> {
    let launcher = &app.state::<AppState>().launcher;
    let front = launcher.current()?;
    let parkable = front.phase == Phase::Running && !front.anti_cheat && !front.game_id.starts_with("app:");
    if !parkable || launcher.suspended().len() >= MAX_SUSPENDED {
        return None;
    }
    let _transition = launcher.transition().ok()?;
    let generation = launcher.foreground_generation()?;
    match swap_out(app) {
        Ok(()) => Some(generation),
        Err(err) => {
            eprintln!("{err:#}");
            None
        }
    }
}

/// Undoes `park_running_game`: thaws the game and hands it the screen again.
fn unpark(app: &AppHandle, generation: u64) {
    let launcher = &app.state::<AppState>().launcher;
    let _transition = launcher.transition_within(TRANSITION_WAIT);
    if launcher.phase(generation) != Some(Phase::Suspended) {
        return;
    }
    if let Err(err) = thaw(app, generation) {
        eprintln!("{err:#}");
    }
    let pids = launcher.running_pids();
    window::resume_game(app, &pids);
}

/// Freezes the foreground game to make room for another one.
fn swap_out(app: &AppHandle) -> Result<()> {
    let launcher = &app.state::<AppState>().launcher;
    let session = launcher.current().context("Nessun gioco in corso")?;
    let generation = launcher.generation_of(&session.game_id).context("Nessun gioco in corso")?;
    let pids = launcher.running_pids();
    let family = freeze::family(&pids);
    crate::wand::forget();
    let hwnd = window::game_window(&pids);
    freeze::suspend(&family, hwnd).map_err(|e| anyhow::anyhow!("Il gioco in corso non si può sospendere: {e:#}"))?;
    let at = now();
    if let Some(state) = launcher.update(generation, |a| {
        a.state.seconds = a.played(at);
        a.state.phase = Phase::Suspended;
        a.state.suspended_at = Some(at);
        a.frozen = family;
    }) {
        heartbeat(launcher, generation);
        let _ = app.emit("game://state", &state);
    }
    Ok(())
}

/// Thaws one frozen session and marks it running again.
fn thaw(app: &AppHandle, generation: u64) -> Result<()> {
    let launcher = &app.state::<AppState>().launcher;
    let frozen = launcher.with(generation, |a| a.frozen.clone()).unwrap_or_default();
    let result = freeze::resume(&frozen);
    let at = now();
    if let Some(state) = launcher.update(generation, |a| {
        if let Some(since) = a.state.suspended_at.take() {
            a.paused_secs += (at - since).max(0) as u64;
        }
        a.frozen.clear();
        a.state.phase = Phase::Running;
        a.state.seconds = a.played(at);
    }) {
        heartbeat(launcher, generation);
        let _ = app.emit("game://state", &state);
    }
    result.map_err(|e| anyhow::anyhow!("Il gioco non è ripartito del tutto: {e:#}"))
}

/// Thaws every frozen game, e.g. before GameMode quits or the PC shuts
/// down: nothing may stay frozen behind GameMode's back.
pub fn thaw_all(app: &AppHandle) {
    crate::overlay_pause::thaw_now();
    let launcher = &app.state::<AppState>().launcher;
    let _transition = launcher.transition_within(TRANSITION_WAIT);
    for session in launcher.suspended() {
        if let Some(generation) = launcher.generation_of(&session.game_id) {
            if let Err(err) = thaw(app, generation) {
                log::warn!("{err:#}");
            }
        }
    }
}

/// Ends a frozen game without thawing it (a frozen process can be killed).
pub fn close_suspended(app: &AppHandle, game_id: &str) -> Result<()> {
    let launcher = &app.state::<AppState>().launcher;
    let _transition = launcher.transition()?;
    let generation = launcher.generation_of(game_id).context("Questo gioco non è in pausa")?;
    if launcher.phase(generation) != Some(Phase::Suspended) {
        anyhow::bail!("Questo gioco non è in pausa");
    }
    let frozen = launcher.with(generation, |a| a.frozen.clone()).unwrap_or_default();
    // The audio engine, not the frozen game, answers this: safe before the kill.
    freeze::unmute(&frozen);
    if !kill(&frozen) {
        anyhow::bail!("Non riesco a chiudere il gioco: riprendilo e chiudilo da lì");
    }
    end(app, generation, Phase::Exited, 0);
    Ok(())
}

/// Terminates `pids`; true when every one that still exists went down.
pub(crate) fn kill(pids: &[u32]) -> bool {
    let mut sys = System::new();
    let targets: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
    sys.refresh_processes(ProcessesToUpdate::Some(&targets), true);
    targets.into_iter().filter_map(|pid| sys.process(pid)).all(|process| process.kill())
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
pub(crate) fn steam_exe() -> Option<PathBuf> {
    crate::library::steam::find_steam_root().map(|root| root.join("steam.exe")).filter(|p| p.is_file())
}

#[cfg(not(windows))]
pub(crate) fn steam_exe() -> Option<PathBuf> {
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
        if let Err(err) = crate::webview2::scrub(Command::new(exe).arg("-silent")).spawn() {
            log::warn!("starting Steam failed: {err}");
        }
    }
}

/// Ends the running game's processes.
pub fn close_running(app: &AppHandle) -> bool {
    let launcher = &app.state::<AppState>().launcher;
    let pids = launcher.running_pids();
    if pids.is_empty() {
        // Still starting: stop waiting for it.
        let generation = launcher.current().and_then(|s| launcher.generation_of(&s.game_id));
        if let Some(generation) = generation {
            end(app, generation, Phase::Exited, 0);
            return true;
        }
        return false;
    }
    kill(&pids);
    true
}

/// Asks every open game to close (WM_CLOSE to its windows, like the X
/// button, so it can save), waits up to `grace` for it, then terminates
/// what is left. Frozen games are thawed first (`thaw_all`) or they can't
/// answer. Returns the ids of the games that were open.
pub fn close_all_gently(app: &AppHandle, grace: Duration) -> Vec<String> {
    let (ids, pids) = app.state::<AppState>().launcher.open_games();
    if pids.is_empty() {
        return ids;
    }
    crate::performance::close_windows(&pids);
    let targets: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
    let deadline = Instant::now() + grace;
    let mut sys = System::new();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        sys.refresh_processes(ProcessesToUpdate::Some(&targets), true);
        if targets.iter().all(|pid| sys.process(*pid).is_none()) {
            return ids;
        }
    }
    // UWP titles and anti-cheat games often ignore WM_CLOSE.
    kill(&pids);
    ids
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
    fn first_run_installers_are_not_the_game() {
        let yes = [
            r"D:\SteamLibrary\steamapps\common\Steamworks Shared\_CommonRedist\vcredist\2019\VC_redist.x64.exe",
            r"D:\SteamLibrary\steamapps\common\Hades\_CommonRedist\DirectX\Jun2010\DXSETUP.exe",
            r"C:\Games\X\Engine\Extras\Redist\en-us\UEPrereqSetup_x64.exe",
            r"C:\Windows\System32\msiexec.exe",
            r"C:\Temp\dotNetFx40_Full_setup.exe",
        ];
        for exe in yes {
            assert!(is_prerequisite(Path::new(exe)), "{exe}");
        }
        let no = [r"D:\SteamLibrary\steamapps\common\Hades\x64\Hades.exe", r"C:\Games\Redistributor\game.exe"];
        for exe in no {
            assert!(!is_prerequisite(Path::new(exe)), "{exe}");
        }
        let target = Target { dir: Some(normalize(Path::new(r"D:\Lib\steamapps\common\Hades"))), family: None, child_pid: None };
        let redist = ProcInfo { exe: Some(r"D:\Lib\steamapps\common\Hades\_CommonRedist\vcredist\VC_redist.x64.exe".into()), family: None };
        let game = ProcInfo { exe: Some(r"D:\Lib\steamapps\common\Hades\Hades.exe".into()), family: None };
        assert!(!target.matches(1, &redist));
        assert!(target.matches(2, &game));
    }

    #[test]
    fn normalizes_windows_paths() {
        assert_eq!(normalize(Path::new(r"C:\Games\Hades\")), "c:/games/hades");
    }

    fn active(phase: Phase) -> Active {
        Active {
            generation: 1,
            state: SessionState {
                game_id: "steam:1".into(),
                game_name: "One".into(),
                phase,
                started_at: 1000,
                seconds: 0,
                update: None,
                suspended_at: None,
                anti_cheat: false,
                stage: None,
            },
            pids: vec![7],
            live_pids: vec![(7, 99)],
            running_since: Some(1000),
            paused_secs: 60,
            frozen: Vec::new(),
        }
    }

    #[test]
    fn play_time_skips_pauses() {
        let mut a = active(Phase::Running);
        assert_eq!(a.played(1300), 240);
        a.state.phase = Phase::Suspended;
        a.state.suspended_at = Some(1300);
        // Frozen: the clock stops.
        assert_eq!(a.played(1900), 240);
        let record = a.live_record(1900).unwrap();
        assert!(record.suspended);
        assert_eq!(record.paused_secs, 660);
        assert_eq!(record.played(), 240);
        a.running_since = None;
        assert!(a.live_record(1900).is_none());
    }

    #[test]
    fn sleep_is_not_play_time() {
        let launcher = Launcher::default();
        let mut frozen = active(Phase::Suspended);
        frozen.generation = 2;
        frozen.state.game_id = "steam:2".into();
        frozen.state.suspended_at = Some(1200);
        launcher.insert(frozen);
        launcher.insert(active(Phase::Running));
        // Running since 1000 with 60 s paused: at 2000 that's 940 s, minus 500 s asleep.
        let touched = launcher.add_pause(500, 2000);
        assert_eq!(touched.len(), 1);
        assert_eq!(touched[0].1.seconds, 440);
        assert_eq!(launcher.current().map(|s| s.seconds), Some(440));
        // The frozen one stays frozen and untouched.
        assert_eq!(launcher.suspended()[0].seconds, 0);
    }

    #[test]
    fn a_new_launch_keeps_the_frozen_games() {
        let launcher = Launcher::default();
        launcher.insert(active(Phase::Running));
        let mut frozen = active(Phase::Suspended);
        frozen.generation = 2;
        frozen.state.game_id = "steam:2".into();
        launcher.insert(frozen);
        let mut next = active(Phase::Launching);
        next.generation = 3;
        next.state.game_id = "steam:3".into();
        launcher.insert(next);
        assert_eq!(launcher.current().map(|s| s.game_id).as_deref(), Some("steam:3"));
        assert_eq!(launcher.suspended().len(), 1);
        assert_eq!(launcher.phase(1), None);
        assert_eq!(launcher.generation_of("steam:2"), Some(2));
        launcher.finish(2);
        assert!(launcher.suspended().is_empty());
    }

    #[test]
    fn frozen_processes_stay_with_their_session() {
        let launcher = Launcher::default();
        let mut frozen = active(Phase::Suspended);
        frozen.frozen = vec![7, 8];
        launcher.insert(frozen);
        let mut app = active(Phase::Running);
        app.generation = 2;
        app.state.game_id = "app:spotify".into();
        launcher.insert(app);
        assert_eq!(launcher.foreground_generation(), Some(2));
        assert_eq!(launcher.frozen_elsewhere(2), vec![7, 8]);
        assert!(launcher.frozen_elsewhere(1).is_empty());
    }
}
