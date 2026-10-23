//! Inactivity and power timers. One thread keeps the PC awake while it
//! matters (a game, a download, a controller in use: Windows only counts
//! keyboard and mouse) and, when nothing happens for a while, steps through
//! Active → Screensaver → DisplayOff → sleep. The same thread runs the
//! "shut down in…" schedule and rest mode (screen off, downloads go on).
//!
//! `SetThreadExecutionState` with `ES_CONTINUOUS` belongs to the calling
//! thread, so everything happens on one dedicated std thread that never ends.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::model::Settings;
use crate::system::PowerAction;
use crate::{launcher, overlay, updates, window, AppState};

const TICK: Duration = Duration::from_secs(1);
/// How often the stores are asked whether something is downloading.
const DOWNLOADS_EVERY: Duration = Duration::from_secs(30);
/// "At the end of the downloads" waits this long with nothing downloading.
const DOWNLOADS_SETTLE: u64 = 60;
/// The countdown shown before a scheduled power action.
pub const WARNING_SECS: u64 = 60;
/// Controller use this recent keeps the display on (Windows ignores pads).
const PAD_KEEPS_DISPLAY_MS: u64 = 2 * 60 * 1000;
/// The press that entered rest mode must not leave it at once.
const REST_GRACE_MS: u64 = 1500;
/// After waking from sleep GameMode takes the screen back once things settle.
const RESUME_FRONT_DELAY: Duration = Duration::from_secs(2);

/// Where the inactivity machine is, as the frontend sees it.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Active,
    Screensaver,
    DisplayOff,
}

impl Stage {
    fn from_u8(v: u8) -> Stage {
        match v {
            1 => Stage::Screensaver,
            2 => Stage::DisplayOff,
            _ => Stage::Active,
        }
    }
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct IdleState {
    stage: Stage,
}

/// When a scheduled power action happens.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PowerMode {
    In { secs: u64 },
    AtDownloadsEnd,
}

/// The pending power action, for the quick menu.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PowerSchedule {
    pub action: PowerAction,
    /// Unix seconds when it happens; unknown while downloads are running.
    pub at: Option<i64>,
    pub at_downloads_end: bool,
}

/// Sent every second during the last `WARNING_SECS`.
#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
struct Countdown {
    action: PowerAction,
    seconds_left: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Scheduled {
    action: PowerAction,
    /// Ms since start; `None` while waiting for the downloads to end.
    deadline: Option<u64>,
    at_downloads_end: bool,
    /// Consecutive seconds with nothing downloading.
    quiet_secs: u64,
    /// The countdown has started (the overlay was shown).
    warned: bool,
}

static SCHEDULE: Mutex<Option<Scheduled>> = Mutex::new(None);
static START: OnceLock<Instant> = OnceLock::new();
/// Ms since start of the last controller press (0 = never).
static LAST_PAD: AtomicU64 = AtomicU64::new(0);
/// Ms since start of the last wake from sleep, or of GameMode's start.
static LAST_RESUME: AtomicU64 = AtomicU64::new(0);
/// Ms since start when rest mode was entered (0 = not resting).
static REST_SINCE: AtomicU64 = AtomicU64::new(0);
static STAGE: AtomicU8 = AtomicU8::new(0);
/// Windows reported a wake from sleep: handled on the next tick.
static RESUMED: AtomicBool = AtomicBool::new(false);
/// The user asked for the screen back (frontend or controller).
static WAKE_REQUESTED: AtomicBool = AtomicBool::new(false);
/// A display-on is already underway for this wake: held buttons and late
/// frontend echoes must not repeat it (it broadcasts and can block).
static DISPLAY_WAKING: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

pub fn stage() -> Stage {
    Stage::from_u8(STAGE.load(Ordering::SeqCst))
}

/// A controller button is down: counts as activity, and brings the screen
/// back when it is off (keyboard and mouse do that by themselves).
pub fn touch_input() {
    LAST_PAD.store(now_ms().max(1), Ordering::SeqCst);
    if stage() == Stage::DisplayOff {
        wake();
    }
}

/// Leaves the screensaver or turns the display back on. The press that
/// just entered rest mode (still held, or its late frontend echo) doesn't.
pub fn wake() {
    let now = now_ms().max(1);
    if now.saturating_sub(REST_SINCE.load(Ordering::SeqCst)) <= REST_GRACE_MS {
        return;
    }
    LAST_PAD.store(now, Ordering::SeqCst);
    WAKE_REQUESTED.store(true, Ordering::SeqCst);
    // Called from the XInput thread and from sync commands (main thread):
    // the display comes back once, on a thread of its own.
    if stage() == Stage::DisplayOff && DISPLAY_WAKING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        std::thread::spawn(imp::display_on);
    }
}

/// Rest mode: screen off right away; downloads go on, any input wakes.
pub fn rest(app: &AppHandle) {
    let now = now_ms().max(1);
    REST_SINCE.store(now, Ordering::SeqCst);
    WAKE_REQUESTED.store(false, Ordering::SeqCst);
    DISPLAY_WAKING.store(false, Ordering::SeqCst);
    set_stage(app, Stage::DisplayOff);
    imp::display_off(app);
}

pub fn schedule(app: &AppHandle, action: PowerAction, mode: PowerMode) -> PowerSchedule {
    let now = now_ms();
    let scheduled = match mode {
        PowerMode::In { secs } => Scheduled { action, deadline: Some(now + secs * 1000), at_downloads_end: false, quiet_secs: 0, warned: false },
        PowerMode::AtDownloadsEnd => Scheduled { action, deadline: None, at_downloads_end: true, quiet_secs: 0, warned: false },
    };
    *SCHEDULE.lock().unwrap() = Some(scheduled);
    let view = scheduled.view(now);
    let _ = app.emit("power://schedule", Some(view));
    view
}

pub fn cancel(app: &AppHandle) {
    let was = SCHEDULE.lock().unwrap().take();
    if was.is_some() {
        let _ = app.emit("power://schedule", None::<PowerSchedule>);
    }
}

pub fn current_schedule() -> Option<PowerSchedule> {
    SCHEDULE.lock().unwrap().map(|s| s.view(now_ms()))
}

impl Scheduled {
    fn view(&self, now: u64) -> PowerSchedule {
        PowerSchedule {
            action: self.action,
            at: self.deadline.map(|d| crate::model::now() + (d.saturating_sub(now) / 1000) as i64),
            at_downloads_end: self.at_downloads_end,
        }
    }

    /// Advances one second; returns what happened this tick.
    fn tick(&mut self, now: u64, downloads_active: bool) -> ScheduleEvent {
        if self.deadline.is_none() {
            self.quiet_secs = if downloads_active { 0 } else { self.quiet_secs + 1 };
            if self.quiet_secs >= DOWNLOADS_SETTLE {
                self.deadline = Some(now + WARNING_SECS * 1000);
                return ScheduleEvent::Armed;
            }
            return ScheduleEvent::Waiting;
        }
        let left_ms = self.deadline.unwrap().saturating_sub(now);
        let left = left_ms.div_ceil(1000);
        if left == 0 {
            return ScheduleEvent::Due;
        }
        if left <= WARNING_SECS {
            let first = !self.warned;
            self.warned = true;
            return ScheduleEvent::Countdown { left, first };
        }
        ScheduleEvent::Waiting
    }
}

/// What the schedule asks for this tick, acted on once its lock is released:
/// showing the overlay waits on the main thread, which may want the lock.
enum ScheduleStep {
    Nothing,
    Armed(PowerSchedule),
    Countdown { action: PowerAction, left: u64, first: bool },
    Due(PowerAction),
}

#[derive(Debug, PartialEq)]
enum ScheduleEvent {
    Waiting,
    /// Downloads ended: the countdown starts.
    Armed,
    Countdown { left: u64, first: bool },
    Due,
}

/// Minutes from Settings, as idle thresholds in seconds (0 = never).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Thresholds {
    screensaver: u64,
    display_off: u64,
    sleep: u64,
}

impl Thresholds {
    fn of(settings: &Settings) -> Thresholds {
        Thresholds {
            screensaver: u64::from(settings.idle_screensaver_min) * 60,
            display_off: u64::from(settings.idle_display_off_min) * 60,
            sleep: u64::from(settings.idle_sleep_min) * 60,
        }
    }
}

/// The stage `idle_secs` of inactivity calls for, timers at 0 never firing.
fn stage_for(idle_secs: u64, t: Thresholds) -> Stage {
    if t.display_off > 0 && idle_secs >= t.display_off {
        Stage::DisplayOff
    } else if t.screensaver > 0 && idle_secs >= t.screensaver {
        Stage::Screensaver
    } else {
        Stage::Active
    }
}

fn sleep_due(idle_secs: u64, t: Thresholds) -> bool {
    t.sleep > 0 && idle_secs >= t.sleep
}

/// What the PC must keep doing: `(system awake, display on)`.
fn required(playing: bool, wait_downloads: bool, pad_recent: bool, in_front: bool, stage: Stage) -> (bool, bool) {
    if playing {
        (true, true)
    } else if stage == Stage::Active && in_front && pad_recent {
        (true, true)
    } else {
        (wait_downloads, false)
    }
}

fn set_stage(app: &AppHandle, stage: Stage) {
    let before = STAGE.swap(stage as u8, Ordering::SeqCst);
    if before != stage as u8 {
        let _ = app.emit("idle://state", IdleState { stage });
    }
}

fn execute(app: &AppHandle, action: PowerAction) {
    // Same path as the quick menu: a shutdown closes and saves the games first.
    if let Err(e) = crate::power_now(app, action) {
        eprintln!("scheduled power action failed: {e:#}");
    }
}

/// Starts the inactivity thread.
pub fn start(app: AppHandle) {
    LAST_RESUME.store(now_ms().max(1), Ordering::SeqCst);
    imp::remember_window(&app);
    imp::watch_resume();
    std::thread::spawn(move || run(app));
}

fn run(app: AppHandle) {
    let state = app.state::<AppState>();
    let mut downloads_active = false;
    let mut downloads_checked: Option<Instant> = None;
    let mut applied: Option<(bool, bool)> = None;

    loop {
        std::thread::sleep(TICK);
        let now = now_ms();
        let settings = state.db.lock().settings.clone();
        let thresholds = Thresholds::of(&settings);

        if downloads_checked.is_none_or(|t| t.elapsed() >= DOWNLOADS_EVERY) {
            downloads_checked = Some(Instant::now());
            let games = state.db.lock().view();
            downloads_active = transferring(&updates::all(&games));
        }

        let playing = state
            .launcher
            .current()
            .is_some_and(|s| matches!(s.phase, launcher::Phase::Running | launcher::Phase::Launching));
        let in_front = window::is_foreground(&app) || overlay::is_open();

        // Back from sleep: start counting again, and take the screen back.
        if RESUMED.swap(false, Ordering::SeqCst) {
            LAST_RESUME.store(now.max(1), Ordering::SeqCst);
            REST_SINCE.store(0, Ordering::SeqCst);
            set_stage(&app, Stage::Active);
            if !playing {
                let h = app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(RESUME_FRONT_DELAY);
                    window::bring_to_front(&h);
                });
            }
        }

        // Scheduled power action. The lock only covers the bookkeeping.
        let (step, schedule_waits, counting_down) = {
            let mut guard = SCHEDULE.lock().unwrap();
            match guard.as_mut() {
                None => (ScheduleStep::Nothing, false, false),
                Some(s) => {
                    let waits = s.at_downloads_end && s.deadline.is_none();
                    let step = match s.tick(now, downloads_active) {
                        ScheduleEvent::Waiting => ScheduleStep::Nothing,
                        ScheduleEvent::Armed => ScheduleStep::Armed(s.view(now)),
                        ScheduleEvent::Countdown { left, first } => ScheduleStep::Countdown { action: s.action, left, first },
                        ScheduleEvent::Due => {
                            let action = s.action;
                            *guard = None;
                            ScheduleStep::Due(action)
                        }
                    };
                    let counting = matches!(step, ScheduleStep::Countdown { .. });
                    (step, waits, counting)
                }
            }
        };
        match step {
            ScheduleStep::Nothing => {}
            ScheduleStep::Armed(view) => {
                let _ = app.emit("power://schedule", Some(view));
            }
            ScheduleStep::Countdown { action, left, first } => {
                let _ = app.emit("power://countdown", Countdown { action, seconds_left: left });
                if first && playing && settings.overlay_enabled && overlay::window(&app).is_some() && !overlay::is_open() {
                    overlay::show(&app);
                }
            }
            ScheduleStep::Due(action) => {
                let _ = app.emit("power://schedule", None::<PowerSchedule>);
                let _ = app.emit("power://countdown", Countdown { action, seconds_left: 0 });
                execute(&app, action);
                continue;
            }
        }
        let wait_downloads = downloads_active && (settings.idle_wait_downloads || schedule_waits);

        // Inactivity.
        let last = last_activity(now);
        let pad_recent = now.saturating_sub(LAST_PAD.load(Ordering::SeqCst)) < PAD_KEEPS_DISPLAY_MS;
        let rest_since = REST_SINCE.load(Ordering::SeqCst);
        let woke = WAKE_REQUESTED.swap(false, Ordering::SeqCst);
        // The last-minute warning must be seen: it ends rest mode too.
        let resting = rest_since > 0 && !woke && !counting_down && last <= rest_since + REST_GRACE_MS;
        if rest_since > 0 && !resting {
            REST_SINCE.store(0, Ordering::SeqCst);
        }
        let idle_secs = now.saturating_sub(if resting { rest_since } else { last }) / 1000;
        let current = stage();

        let target = if resting {
            Stage::DisplayOff
        } else if playing || wait_downloads || !in_front || woke || counting_down {
            Stage::Active
        } else {
            stage_for(idle_secs, thresholds)
        };
        if target != current {
            match (current, target) {
                (_, Stage::DisplayOff) => {
                    DISPLAY_WAKING.store(false, Ordering::SeqCst);
                    imp::display_off(&app);
                }
                // A wake() may have turned it on already.
                (Stage::DisplayOff, _) => {
                    if !DISPLAY_WAKING.swap(false, Ordering::SeqCst) {
                        imp::display_on();
                    }
                }
                _ => {}
            }
            set_stage(&app, target);
        }
        if !playing && !wait_downloads && (resting || in_front) && sleep_due(idle_secs, thresholds) {
            // Count again from here, in case the PC doesn't go (or comes right back).
            LAST_RESUME.store(now.max(1), Ordering::SeqCst);
            REST_SINCE.store(0, Ordering::SeqCst);
            execute(&app, settings.idle_action);
            continue;
        }

        let wanted = required(playing, wait_downloads, pad_recent, in_front, target);
        if applied != Some(wanted) {
            applied = Some(wanted);
            imp::set_execution_state(wanted.0, wanted.1);
        }
    }
}

/// Something is being downloaded or installed right now. Paused downloads
/// don't count: they can stay paused for days.
fn transferring(list: &[updates::GameUpdate]) -> bool {
    list.iter().any(|u| matches!(u.state, updates::UpdateState::Downloading | updates::UpdateState::Installing))
}

/// Ms since start of the most recent activity GameMode knows about.
fn last_activity(now: u64) -> u64 {
    let os = imp::os_idle_ms().map_or(0, |age| now.saturating_sub(age));
    os.max(LAST_PAD.load(Ordering::SeqCst)).max(LAST_RESUME.load(Ordering::SeqCst))
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicIsize, Ordering};

    use tauri::AppHandle;
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
    use windows::Win32::System::Power::{
        PowerRegisterSuspendResumeNotification, SetThreadExecutionState, DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS,
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED,
    };
    use windows::Win32::System::SystemInformation::GetTickCount64;
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetLastInputInfo, SendInput, INPUT, INPUT_0, INPUT_MOUSE, LASTINPUTINFO, MOUSEEVENTF_MOVE, MOUSEINPUT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, DEVICE_NOTIFY_CALLBACK, HWND_BROADCAST, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND,
        SC_MONITORPOWER, SMTO_ABORTIFHUNG, WM_SYSCOMMAND,
    };

    const MONITOR_OFF: isize = 2;
    const MONITOR_ON: isize = -1;
    const SEND_TIMEOUT_MS: u32 = 2000;

    /// The main window's HWND, so no thread has to ask the event loop for it
    /// (0 = unknown yet: the message is broadcast).
    static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

    pub fn remember_window(app: &AppHandle) {
        if let Some(h) = crate::window::main_window(app).and_then(|w| w.hwnd().ok()) {
            MAIN_HWND.store(h.0 as isize, Ordering::SeqCst);
        }
    }

    /// Ms since the last keyboard or mouse input (controllers don't count).
    pub fn os_idle_ms() -> Option<u64> {
        let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        unsafe {
            if !GetLastInputInfo(&mut info).as_bool() {
                return None;
            }
            // `dwTime` is a 32-bit tick count: wrap-around safe subtraction.
            let now = GetTickCount64() as u32;
            Some(u64::from(now.wrapping_sub(info.dwTime)))
        }
    }

    pub fn set_execution_state(system: bool, display: bool) {
        let mut flags = ES_CONTINUOUS;
        if system {
            flags |= ES_SYSTEM_REQUIRED;
        }
        if display {
            flags |= ES_DISPLAY_REQUIRED;
        }
        unsafe {
            SetThreadExecutionState(flags);
        }
    }

    fn monitor_power(state: isize) {
        let hwnd = MAIN_HWND.load(Ordering::SeqCst);
        let target = if hwnd != 0 { HWND(hwnd as *mut core::ffi::c_void) } else { HWND_BROADCAST };
        unsafe {
            SendMessageTimeoutW(
                target,
                WM_SYSCOMMAND,
                WPARAM(SC_MONITORPOWER as usize),
                LPARAM(state),
                SMTO_ABORTIFHUNG,
                SEND_TIMEOUT_MS,
                None,
            );
        }
    }

    pub fn display_off(app: &AppHandle) {
        if MAIN_HWND.load(Ordering::SeqCst) == 0 {
            remember_window(app);
        }
        monitor_power(MONITOR_OFF);
    }

    /// `SC_MONITORPOWER -1` alone is unreliable: a tiny mouse move counts as
    /// real input and wakes the display the way Windows expects.
    pub fn display_on() {
        let mut inputs = [INPUT::default(); 2];
        for (input, dx) in inputs.iter_mut().zip([1, -1]) {
            input.r#type = INPUT_MOUSE;
            input.Anonymous = INPUT_0 { mi: MOUSEINPUT { dx, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 } };
        }
        unsafe {
            SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
        }
        monitor_power(MONITOR_ON);
    }

    unsafe extern "system" fn on_power_event(_ctx: *const core::ffi::c_void, kind: u32, _setting: *const core::ffi::c_void) -> u32 {
        if kind == PBT_APMRESUMEAUTOMATIC || kind == PBT_APMRESUMESUSPEND {
            super::RESUMED.store(true, Ordering::SeqCst);
        }
        0
    }

    /// Hears the PC wake from sleep; no window needed.
    pub fn watch_resume() {
        let params = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(on_power_event),
            Context: std::ptr::null_mut(),
        }));
        let mut handle = std::ptr::null_mut();
        let err = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                HANDLE(params as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS as *mut core::ffi::c_void),
                &mut handle,
            )
        };
        if err.0 != 0 {
            eprintln!("suspend/resume notifications unavailable: {}", err.0);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use tauri::AppHandle;

    /// Unknown outside Windows: only controller and frontend pings count.
    pub fn os_idle_ms() -> Option<u64> {
        None
    }

    pub fn set_execution_state(_system: bool, _display: bool) {}

    pub fn remember_window(_app: &AppHandle) {}

    pub fn display_off(_app: &AppHandle) {
        eprintln!("display off ignored outside Windows");
    }

    pub fn display_on() {}

    pub fn watch_resume() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Thresholds = Thresholds { screensaver: 600, display_off: 1800, sleep: 3600 };

    #[test]
    fn stages_follow_the_thresholds() {
        assert_eq!(stage_for(0, T), Stage::Active);
        assert_eq!(stage_for(599, T), Stage::Active);
        assert_eq!(stage_for(600, T), Stage::Screensaver);
        assert_eq!(stage_for(1799, T), Stage::Screensaver);
        assert_eq!(stage_for(1800, T), Stage::DisplayOff);
        assert!(!sleep_due(3599, T));
        assert!(sleep_due(3600, T));
    }

    #[test]
    fn zero_means_never() {
        let never = Thresholds::default();
        assert_eq!(stage_for(u64::MAX, never), Stage::Active);
        assert!(!sleep_due(u64::MAX, never));
        // Screensaver off, display off still on.
        let t = Thresholds { screensaver: 0, display_off: 60, sleep: 0 };
        assert_eq!(stage_for(59, t), Stage::Active);
        assert_eq!(stage_for(60, t), Stage::DisplayOff);
        assert!(!sleep_due(u64::MAX, t));
    }

    #[test]
    fn thresholds_come_from_minutes() {
        let settings = Settings { idle_screensaver_min: 1, idle_display_off_min: 2, idle_sleep_min: 0, ..Settings::default() };
        assert_eq!(Thresholds::of(&settings), Thresholds { screensaver: 60, display_off: 120, sleep: 0 });
    }

    #[test]
    fn a_game_keeps_everything_on() {
        assert_eq!(required(true, false, false, false, Stage::DisplayOff), (true, true));
    }

    #[test]
    fn a_recent_pad_press_keeps_the_display_on_only_in_front() {
        assert_eq!(required(false, false, true, true, Stage::Active), (true, true));
        assert_eq!(required(false, false, true, false, Stage::Active), (false, false));
        assert_eq!(required(false, false, true, true, Stage::Screensaver), (false, false));
    }

    #[test]
    fn downloads_keep_only_the_system_awake() {
        assert_eq!(required(false, true, false, true, Stage::DisplayOff), (true, false));
        assert_eq!(required(false, false, false, true, Stage::DisplayOff), (false, false));
    }

    #[test]
    fn timed_schedule_counts_down_in_the_last_minute() {
        let mut s = Scheduled { action: PowerAction::Shutdown, deadline: Some(90_000), at_downloads_end: false, quiet_secs: 0, warned: false };
        assert_eq!(s.tick(0, false), ScheduleEvent::Waiting);
        assert_eq!(s.tick(29_500, false), ScheduleEvent::Waiting);
        assert_eq!(s.tick(30_000, false), ScheduleEvent::Countdown { left: 60, first: true });
        assert_eq!(s.tick(31_000, false), ScheduleEvent::Countdown { left: 59, first: false });
        assert_eq!(s.tick(89_200, false), ScheduleEvent::Countdown { left: 1, first: false });
        assert_eq!(s.tick(90_000, false), ScheduleEvent::Due);
        assert_eq!(s.tick(95_000, false), ScheduleEvent::Due);
    }

    #[test]
    fn downloads_end_schedule_waits_for_a_quiet_minute() {
        let mut s = Scheduled { action: PowerAction::Sleep, deadline: None, at_downloads_end: true, quiet_secs: 0, warned: false };
        for t in 0..30 {
            assert_eq!(s.tick(t * 1000, true), ScheduleEvent::Waiting);
        }
        for t in 30..89 {
            assert_eq!(s.tick(t * 1000, false), ScheduleEvent::Waiting);
        }
        // A download coming back resets the quiet time.
        assert_eq!(s.tick(89_000, true), ScheduleEvent::Waiting);
        assert_eq!(s.quiet_secs, 0);
        let mut t = 90;
        loop {
            let ev = s.tick(t * 1000, false);
            if ev == ScheduleEvent::Armed {
                break;
            }
            assert_eq!(ev, ScheduleEvent::Waiting);
            t += 1;
        }
        assert_eq!(t, 90 + DOWNLOADS_SETTLE - 1);
        assert_eq!(s.deadline, Some(t * 1000 + WARNING_SECS * 1000));
        assert_eq!(s.tick(t * 1000 + 1000, false), ScheduleEvent::Countdown { left: 59, first: true });
    }

    #[test]
    fn paused_downloads_dont_keep_the_pc_awake() {
        use updates::{GameUpdate, UpdateState};
        let of = |state| GameUpdate { game_id: "g".into(), state, done: 0, total: 0, progress: None };
        assert!(!transferring(&[of(UpdateState::Paused), of(UpdateState::Available)]));
        assert!(transferring(&[of(UpdateState::Paused), of(UpdateState::Downloading)]));
        assert!(transferring(&[of(UpdateState::Installing)]));
        assert!(!transferring(&[]));
    }

    #[test]
    fn schedule_view_reports_unix_time_only_when_known() {
        let waiting = Scheduled { action: PowerAction::Sleep, deadline: None, at_downloads_end: true, quiet_secs: 0, warned: false };
        assert_eq!(waiting.view(0).at, None);
        assert!(waiting.view(0).at_downloads_end);
        let timed = Scheduled { action: PowerAction::Shutdown, deadline: Some(120_000), at_downloads_end: false, quiet_secs: 0, warned: false };
        let at = timed.view(0).at.unwrap();
        let unix = crate::model::now();
        assert!((at - unix - 120).abs() <= 1);
    }
}
