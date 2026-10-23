//! Console-style life cycle: what GameMode does when the PC goes to sleep
//! and wakes up, when the session is locked and unlocked, when the display
//! changes and when Windows signs out — plus the idle timer that puts the
//! PC to sleep by itself when nobody touches keyboard, mouse or controller.
//!
//! Windows tells a top-level window about all of this, so a hidden one runs
//! on its own thread with its own message loop; every reaction happens in
//! the platform-neutral code below, which the frontend hears about through
//! `power://state`.

// The platform layer only exists on Windows; the rest stays testable anywhere.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::{launcher, model::now, overlay, performance, shell, system, window, AppState};

/// What the frontend hears about.
#[derive(Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PowerEvent {
    /// The PC is going to sleep.
    Suspended,
    /// Back from sleep, after `slept` seconds.
    Resumed { slept: u64 },
    Locked,
    Unlocked,
    DisplayOff,
    DisplayOn,
    /// The idle timer puts the PC to sleep in `seconds` unless someone moves.
    SleepSoon { seconds: u64 },
    SleepCancelled,
    /// Windows is signing out or shutting down.
    SessionEnding,
}

/// What the system says, platform-neutral.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Signal {
    Suspend,
    Resume,
    Lock,
    Unlock,
    /// The console display went on (true) or off.
    Display(bool),
    /// The display mode changed (a game, a TV, Windows after a wake).
    DisplayChange,
    SessionEnding,
}

static APP: OnceLock<AppHandle> = OnceLock::new();
static SUSPENDED: AtomicBool = AtomicBool::new(false);
static LOCKED: AtomicBool = AtomicBool::new(false);
static ENDING: AtomicBool = AtomicBool::new(false);
static DISPLAY_OFF: AtomicBool = AtomicBool::new(false);
/// Bumped on every wake: game monitors give their game a fresh grace period.
static WAKES: AtomicU64 = AtomicU64::new(0);
/// Unix seconds of the last suspend and of the last wake (0: never).
static SLEPT_AT: AtomicI64 = AtomicI64::new(0);
static WOKE_AT: AtomicI64 = AtomicI64::new(0);
/// GameMode (or its overlay) had the screen when the PC went to sleep.
static HAD_SCREEN: AtomicBool = AtomicBool::new(false);
/// The wake happened behind the lock screen: the unlock brings GameMode back.
static WOKE_LOCKED: AtomicBool = AtomicBool::new(false);
/// Coalesces bursts of display-change messages.
static DISPLAY_CHANGES: AtomicU64 = AtomicU64::new(0);

/// How long after a wake or an unlock before GameMode comes back, and the
/// later retries (TVs re-enumerate over HDMI slowly, DWM takes a moment).
const COME_BACK_DELAYS: [u64; 3] = [2, 4, 6];
/// Display changes settle for this long before TV mode is checked.
const DISPLAY_SETTLE: Duration = Duration::from_millis(1500);
/// How often the idle timer looks.
const IDLE_POLL: Duration = Duration::from_secs(30);
/// Warning before the automatic sleep, in seconds.
pub const WARN_BEFORE: u64 = 60;

/// Starts listening to the system and the idle timer.
pub fn start(app: &AppHandle) {
    let _ = APP.set(app.clone());
    imp::start();
    let handle = app.clone();
    std::thread::spawn(move || idle_loop(&handle));
}

pub fn locked() -> bool {
    LOCKED.load(Ordering::SeqCst)
}

pub fn suspended() -> bool {
    SUSPENDED.load(Ordering::SeqCst)
}

/// Windows is signing out or shutting down: no coming back to the screen.
pub fn session_ending() -> bool {
    ENDING.load(Ordering::SeqCst)
}

/// How many times the PC woke up so far.
pub fn wakes() -> u64 {
    WAKES.load(Ordering::SeqCst)
}

fn emit(event: PowerEvent) {
    if let Some(app) = APP.get() {
        let _ = app.emit("power://state", event);
    }
}

/// Entry point for the platform layer.
pub fn signal(signal: Signal) {
    let Some(app) = APP.get() else { return };
    match signal {
        Signal::Suspend => on_suspend(app),
        Signal::Resume => on_resume(app),
        Signal::Lock => on_lock(app),
        Signal::Unlock => on_unlock(app),
        Signal::Display(on) => {
            DISPLAY_OFF.store(!on, Ordering::SeqCst);
            emit(if on { PowerEvent::DisplayOn } else { PowerEvent::DisplayOff });
        }
        Signal::DisplayChange => on_display_change(app),
        Signal::SessionEnding => on_session_ending(app),
    }
}

fn on_suspend(app: &AppHandle) {
    if SUSPENDED.swap(true, Ordering::SeqCst) {
        return;
    }
    SLEPT_AT.store(now(), Ordering::SeqCst);
    HAD_SCREEN.store(window::is_foreground(app) || overlay::is_open(), Ordering::SeqCst);
    overlay::hide(app);
    // The previous power plan comes back; the game is paused by the sleep anyway.
    performance::end();
    emit(PowerEvent::Suspended);
}

fn on_resume(app: &AppHandle) {
    // Both "automatic" and "user present" resumes arrive: one wake.
    if !SUSPENDED.swap(false, Ordering::SeqCst) {
        return;
    }
    let at = now();
    let slept_at = SLEPT_AT.swap(0, Ordering::SeqCst);
    let slept = if slept_at > 0 { (at - slept_at).max(0) as u64 } else { 0 };
    WOKE_AT.store(at, Ordering::SeqCst);
    WAKES.fetch_add(1, Ordering::SeqCst);
    // A night in sleep isn't play time.
    launcher::after_sleep(app, slept);
    let state = app.state::<AppState>();
    let performance_mode = state.db.lock().settings.performance_mode;
    if performance_mode && state.launcher.current().is_some() {
        // `end` at suspend restored the old plan; the game is still there.
        performance::begin(&[]);
    }
    emit(PowerEvent::Resumed { slept });
    if locked() {
        WOKE_LOCKED.store(true, Ordering::SeqCst);
        return;
    }
    come_back(app, HAD_SCREEN.load(Ordering::SeqCst));
}

fn on_lock(app: &AppHandle) {
    if LOCKED.swap(true, Ordering::SeqCst) {
        return;
    }
    overlay::hide(app);
    emit(PowerEvent::Locked);
}

fn on_unlock(app: &AppHandle) {
    if !LOCKED.swap(false, Ordering::SeqCst) {
        return;
    }
    emit(PowerEvent::Unlocked);
    let after_wake = WOKE_LOCKED.swap(false, Ordering::SeqCst);
    // A plain Win+L on the desktop: GameMode stays where it was, unless it
    // is the shell (nothing else draws the screen) or the lock covered a wake.
    if shell::status().active || after_wake {
        come_back(app, after_wake && HAD_SCREEN.load(Ordering::SeqCst));
    }
}

/// Whether GameMode should take the screen back after a wake or an unlock:
/// always as the shell; on the desktop only when it had the screen before
/// or a game is up (so "Riprendi" is one press away), and never over a
/// full-screen app in front (a game that survived and got the screen back).
fn should_come_back(app: &AppHandle, had_screen: bool) -> bool {
    if session_ending() || suspended() || locked() {
        return false;
    }
    let console = shell::status().active;
    let game_up = app.state::<AppState>().launcher.current().is_some();
    (console || had_screen || game_up) && !window::fullscreen_app_in_front(app)
}

/// Brings GameMode back on screen with TV mode and audio, retrying while
/// displays and DWM settle.
fn come_back(app: &AppHandle, had_screen: bool) {
    let app = app.clone();
    std::thread::spawn(move || {
        for (i, delay) in COME_BACK_DELAYS.iter().enumerate() {
            std::thread::sleep(Duration::from_secs(*delay));
            if !should_come_back(&app, had_screen) {
                return;
            }
            // The first pass always comes back; later ones only fix what is
            // still off, so a game resumed meanwhile is left alone.
            if i == 0 || !window::is_foreground(&app) {
                window::bring_to_front(&app);
            } else if tv_mode_off(&app) {
                window::apply_tv(&app);
            }
        }
    });
}

/// TV mode is configured but the display runs another mode.
fn tv_mode_off(app: &AppHandle) -> bool {
    let wanted = app.state::<AppState>().db.lock().settings.tv_mode;
    window::wants_fullscreen(app) && wanted.is_some_and(|m| crate::display::current_mode() != Some(m))
}

fn on_display_change(app: &AppHandle) {
    let generation = DISPLAY_CHANGES.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(DISPLAY_SETTLE);
        // Only the last of a burst looks.
        if DISPLAY_CHANGES.load(Ordering::SeqCst) != generation || session_ending() || suspended() {
            return;
        }
        // Games change the display mode themselves: never fight them.
        if app.state::<AppState>().launcher.current().is_some() {
            return;
        }
        if tv_mode_off(&app) {
            window::apply_tv(&app);
        }
        if overlay::is_open() {
            overlay::fit_screen(&app);
        }
    });
}

fn on_session_ending(app: &AppHandle) {
    if ENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    // Frozen games can't answer the sign-out.
    launcher::thaw_all(app);
    crate::display::reset_mode();
    emit(PowerEvent::SessionEnding);
}

/* ---------- Keeping the PC awake during a game ---------- */

/// Holds the PC and the display awake for as long as it is on: Windows
/// doesn't count controller input as activity, so a game played with the
/// pad alone would otherwise go to sleep mid-session. Per thread: the game
/// monitor owns one.
#[derive(Default)]
pub struct Awake {
    on: bool,
}

impl Awake {
    pub fn set(&mut self, on: bool) {
        if self.on != on {
            self.on = on;
            imp::keep_awake(on);
        }
    }
}

impl Drop for Awake {
    fn drop(&mut self) {
        self.set(false);
    }
}

/* ---------- Idle timer ---------- */

/// What the idle timer does this round.
#[derive(PartialEq, Debug)]
pub enum Step {
    Nothing,
    /// Say that sleep comes in this many seconds.
    Warn(u64),
    /// The warning no longer applies.
    Cancel,
    Sleep,
}

/// Decides from the idle time, the configured threshold and whether the
/// warning was already given.
pub fn step(idle: u64, threshold: u64, warned: bool) -> Step {
    if idle >= threshold {
        Step::Sleep
    } else if idle + WARN_BEFORE >= threshold {
        if warned { Step::Nothing } else { Step::Warn(threshold - idle) }
    } else if warned {
        Step::Cancel
    } else {
        Step::Nothing
    }
}

/// Seconds since the last activity, merging what Windows counts (keyboard,
/// mouse) with the controller and the last wake, which Windows doesn't.
/// Unknown on every side means "not idle".
pub fn idle_secs(system_idle: Option<u64>, pad_last: i64, woke_at: i64, at: i64) -> u64 {
    let since = |t: i64| (t > 0).then(|| (at - t).max(0) as u64);
    [system_idle, since(pad_last), since(woke_at)].into_iter().flatten().min().unwrap_or(0)
}

fn idle_loop(app: &AppHandle) {
    let mut warned = false;
    loop {
        std::thread::sleep(IDLE_POLL);
        let state = app.state::<AppState>();
        let minutes = state.db.lock().settings.auto_sleep_minutes;
        let threshold = minutes.filter(|m| *m > 0).map(|m| u64::from(m) * 60);
        // Off, asleep, locked, signing out, or a game is up: no timer.
        let busy = suspended() || locked() || session_ending() || state.launcher.current().is_some();
        let Some(threshold) = threshold.filter(|_| !busy) else {
            if std::mem::take(&mut warned) {
                emit(PowerEvent::SleepCancelled);
            }
            continue;
        };
        let idle = idle_secs(imp::system_idle_secs(), crate::gamepad::last_input(), WOKE_AT.load(Ordering::SeqCst), now());
        match step(idle, threshold, warned) {
            Step::Nothing => {}
            Step::Warn(seconds) => {
                warned = true;
                emit(PowerEvent::SleepSoon { seconds });
            }
            Step::Cancel => {
                warned = false;
                emit(PowerEvent::SleepCancelled);
            }
            Step::Sleep => {
                warned = false;
                emit(PowerEvent::SleepCancelled);
                if let Err(err) = system::power(system::PowerAction::Sleep) {
                    eprintln!("auto sleep: {err:#}");
                }
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows::core::w;
    use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Power::{
        RegisterPowerSettingNotification, RegisterSuspendResumeNotification, SetThreadExecutionState,
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, POWERBROADCAST_SETTING,
    };
    use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage,
        DEVICE_NOTIFY_WINDOW_HANDLE, MSG, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND, PBT_APMSUSPEND,
        PBT_POWERSETTINGCHANGE, WINDOW_EX_STYLE, WM_DISPLAYCHANGE, WM_ENDSESSION, WM_POWERBROADCAST,
        WM_QUERYENDSESSION, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_OVERLAPPED, WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    };

    use super::{signal, Signal};

    /// A hidden top-level window on its own thread: broadcasts (power,
    /// display, end of session) only reach top-level windows, and the
    /// registrations below make the targeted ones certain.
    pub fn start() {
        std::thread::spawn(message_loop);
    }

    fn message_loop() {
        unsafe {
            let instance = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
            let class = w!("GameModeLifecycle");
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: class, ..Default::default() };
            if RegisterClassW(&wc) == 0 {
                eprintln!("lifecycle window class not registered");
                return;
            }
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("GameMode"),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            ) {
                Ok(hwnd) => hwnd,
                Err(err) => {
                    eprintln!("lifecycle window: {err}");
                    return;
                }
            };
            let recipient = HANDLE(hwnd.0);
            if let Err(err) = RegisterSuspendResumeNotification(recipient, DEVICE_NOTIFY_WINDOW_HANDLE) {
                eprintln!("suspend/resume notifications: {err}");
            }
            if let Err(err) = RegisterPowerSettingNotification(recipient, &GUID_CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_WINDOW_HANDLE) {
                eprintln!("display state notifications: {err}");
            }
            // Needs the Remote Desktop Services service; without it there is
            // simply no lock/unlock news.
            if let Err(err) = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) {
                eprintln!("session notifications: {err}");
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_POWERBROADCAST => {
                match wparam.0 as u32 {
                    PBT_APMSUSPEND => signal(Signal::Suspend),
                    PBT_APMRESUMEAUTOMATIC | PBT_APMRESUMESUSPEND => signal(Signal::Resume),
                    PBT_POWERSETTINGCHANGE => {
                        let setting = lparam.0 as *const POWERBROADCAST_SETTING;
                        if !setting.is_null() {
                            let setting = unsafe { &*setting };
                            if setting.PowerSetting == GUID_CONSOLE_DISPLAY_STATE && setting.DataLength >= 1 {
                                signal(Signal::Display(setting.Data[0] != 0));
                            }
                        }
                    }
                    _ => {}
                }
                LRESULT(1)
            }
            WM_WTSSESSION_CHANGE => {
                match wparam.0 as u32 {
                    WTS_SESSION_LOCK => signal(Signal::Lock),
                    WTS_SESSION_UNLOCK => signal(Signal::Unlock),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DISPLAYCHANGE => {
                signal(Signal::DisplayChange);
                LRESULT(0)
            }
            WM_QUERYENDSESSION => LRESULT(1),
            WM_ENDSESSION => {
                // Synchronous on purpose: the process is killed soon after.
                if wparam.0 != 0 {
                    signal(Signal::SessionEnding);
                }
                LRESULT(0)
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    pub fn keep_awake(on: bool) {
        let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
        unsafe {
            SetThreadExecutionState(flags);
        }
    }

    /// Seconds since the last keyboard or mouse input, as Windows counts it.
    pub fn system_idle_secs() -> Option<u64> {
        let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        unsafe {
            if !GetLastInputInfo(&mut info).as_bool() {
                return None;
            }
            // Both are 32-bit tick counts: the difference survives the wrap.
            Some(u64::from(GetTickCount().wrapping_sub(info.dwTime)) / 1000)
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn start() {}

    pub fn keep_awake(_on: bool) {}

    pub fn system_idle_secs() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_a_minute_before_and_sleeps_at_the_threshold() {
        let threshold = 15 * 60;
        assert_eq!(step(100, threshold, false), Step::Nothing);
        assert_eq!(step(threshold - 60, threshold, false), Step::Warn(60));
        assert_eq!(step(threshold - 30, threshold, true), Step::Nothing);
        // Someone moved: the warning goes away.
        assert_eq!(step(5, threshold, true), Step::Cancel);
        assert_eq!(step(threshold, threshold, true), Step::Sleep);
        assert_eq!(step(threshold + 1000, threshold, false), Step::Sleep);
    }

    #[test]
    fn idle_is_the_most_recent_activity_of_any_kind() {
        // Keyboard idle for an hour, pad used 2 minutes ago.
        assert_eq!(idle_secs(Some(3600), 1_000_000 - 120, 0, 1_000_000), 120);
        // Pad never used: Windows' count stands.
        assert_eq!(idle_secs(Some(3600), 0, 0, 1_000_000), 3600);
        // Just woke up by the power button: not idle, whatever Windows says.
        assert_eq!(idle_secs(Some(3600), 0, 1_000_000 - 10, 1_000_000), 10);
        // Nothing known: never sleep on a guess.
        assert_eq!(idle_secs(None, 0, 0, 1_000_000), 0);
    }
}
