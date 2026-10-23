//! Native controller input. XInput is polled from a background thread so the
//! Guide button can summon the launcher while a game has focus — something
//! the WebView's Gamepad API can't do.
// Parsers and helpers below are only reached from Windows-only code paths.
#![cfg_attr(not(windows), allow(dead_code))]

use serde::Serialize;
use tauri::AppHandle;

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    Menu,
    View,
    X,
    Y,
    Lb,
    Rb,
    Lt,
    Rt,
    Guide,
}

#[derive(Serialize, Clone)]
struct NavEvent {
    action: Action,
    repeat: bool,
}

/// A connected controller, for the battery indicator.
#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Controller {
    pub slot: u32,
    pub wired: bool,
    /// 0 empty … 3 full; meaningless when wired.
    pub level: u8,
}

static CONTROLLERS: std::sync::Mutex<Vec<Controller>> = std::sync::Mutex::new(Vec::new());

pub fn controllers() -> Vec<Controller> {
    CONTROLLERS.lock().unwrap().clone()
}

/// Unix seconds of the last controller activity (buttons, sticks or
/// triggers moved); 0 before any. Windows doesn't count the pad as input,
/// so the idle timer asks here.
static LAST_INPUT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn last_input() -> i64 {
    LAST_INPUT.load(std::sync::atomic::Ordering::Relaxed)
}

fn note_input() {
    LAST_INPUT.store(crate::model::now(), std::sync::atomic::Ordering::Relaxed);
}

/// What the idle timer looks at on one controller.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct PadSnapshot {
    pub buttons: u16,
    pub sticks: [i16; 4],
    pub triggers: [u8; 2],
}

/// How far a stick axis or a trigger has to travel from where it last
/// counted as activity: drift and noise (even a worn stick resting near
/// any given value) never get there, a real push always does.
const STICK_TRAVEL: i32 = 4_096;
const TRIGGER_TRAVEL: i32 = 32;

impl PadSnapshot {
    /// Someone touched the controller since `before` (the last state that
    /// counted as activity): a button changed or an axis travelled far enough.
    pub fn moved_from(&self, before: &PadSnapshot) -> bool {
        self.buttons != before.buttons
            || self.sticks.iter().zip(before.sticks).any(|(a, b)| (i32::from(*a) - i32::from(b)).abs() >= STICK_TRAVEL)
            || self.triggers.iter().zip(before.triggers).any(|(a, b)| (i32::from(*a) - i32::from(b)).abs() >= TRIGGER_TRAVEL)
    }
}

/// Starts the polling thread. Returns false when no native backend exists on
/// this platform, so the frontend falls back to the Gamepad API.
pub fn start(app: AppHandle) -> bool {
    #[cfg(windows)]
    {
        xinput::start(app);
        true
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        false
    }
}

/// B + View held on a controller while GameMode starts, like a console's
/// "boot to desktop" hold. Read once, before the polling thread runs; a
/// wired pad or a dongle works, a Bluetooth pad isn't connected yet at sign-in.
pub fn desktop_combo_held() -> bool {
    #[cfg(windows)]
    {
        xinput::combo_held_at_startup(xinput::DESKTOP_COMBO)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Edge detection and key repeat over a merged button bitmask.
pub struct Repeater {
    prev: u32,
    held_since: [Option<std::time::Instant>; 32],
    last_fire: [Option<std::time::Instant>; 32],
}

pub const REPEAT_DELAY: std::time::Duration = std::time::Duration::from_millis(400);
pub const REPEAT_RATE: std::time::Duration = std::time::Duration::from_millis(80);

impl Default for Repeater {
    fn default() -> Self {
        Repeater { prev: 0, held_since: [None; 32], last_fire: [None; 32] }
    }
}

impl Repeater {
    /// Returns `(bit, is_repeat)` for every button that fires this frame.
    /// Only bits in `repeatable` auto-repeat while held.
    pub fn step(&mut self, buttons: u32, repeatable: u32, now: std::time::Instant) -> Vec<(u32, bool)> {
        let mut fired = Vec::new();
        for i in 0..32 {
            let bit = 1u32 << i;
            let down = buttons & bit != 0;
            let was_down = self.prev & bit != 0;
            if down && !was_down {
                self.held_since[i] = Some(now);
                self.last_fire[i] = Some(now);
                fired.push((bit, false));
            } else if down && repeatable & bit != 0 {
                let held = self.held_since[i].map_or(false, |t| now - t >= REPEAT_DELAY);
                let due = self.last_fire[i].map_or(true, |t| now - t >= REPEAT_RATE);
                if held && due {
                    self.last_fire[i] = Some(now);
                    fired.push((bit, true));
                }
            } else if !down {
                self.held_since[i] = None;
            }
        }
        self.prev = buttons;
        fired
    }
}

#[cfg(windows)]
mod xinput {
    use std::time::{Duration, Instant};

    use tauri::{AppHandle, Emitter, Manager};
    use windows::core::{s, PCSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    use windows::Win32::UI::Input::XboxController::{
        XInputGetBatteryInformation, XInputGetState, XInputSetState, BATTERY_DEVTYPE_GAMEPAD,
        BATTERY_TYPE_DISCONNECTED, BATTERY_TYPE_WIRED, XINPUT_BATTERY_INFORMATION, XINPUT_STATE, XINPUT_VIBRATION,
    };

    use super::{Action, Controller, NavEvent, Repeater, CONTROLLERS};
    use crate::assist::{self, Key};
    use crate::{keyboard, overlay, window};

    const DPAD_UP: u32 = 0x0001;
    const DPAD_DOWN: u32 = 0x0002;
    const DPAD_LEFT: u32 = 0x0004;
    const DPAD_RIGHT: u32 = 0x0008;
    const START: u32 = 0x0010;
    const BACK: u32 = 0x0020;
    const LB: u32 = 0x0100;
    const RB: u32 = 0x0200;
    const GUIDE: u32 = 0x0400;
    const A: u32 = 0x1000;
    const B: u32 = 0x2000;
    const X: u32 = 0x4000;
    const Y: u32 = 0x8000;
    // Virtual bits beyond XInput's 16.
    const LT: u32 = 0x1_0000;
    const RT: u32 = 0x2_0000;

    const DIRECTIONS: u32 = DPAD_UP | DPAD_DOWN | DPAD_LEFT | DPAD_RIGHT;
    const STICK_THRESHOLD: i16 = 16_000;
    const TRIGGER_THRESHOLD: u8 = 128;
    const COMBO_HOLD: Duration = Duration::from_secs(1);
    /// LB+RB hold that opens Wand's overlay during a game.
    const WAND_HOLD: Duration = Duration::from_millis(800);
    /// How often to check that Wand still has the keyboard.
    const WAND_CHECK: Duration = Duration::from_millis(200);
    /// During a game Back+Start toggles the overlay quickly: unlike Guide,
    /// neither Steam nor Game Bar grab this combination.
    const COMBO_HOLD_IN_GAME: Duration = Duration::from_millis(300);
    const POLL: Duration = Duration::from_millis(8);
    /// Polling an empty slot is slow, so disconnected slots are probed rarely.
    const PROBE_INTERVAL: Duration = Duration::from_secs(1);
    const BATTERY_INTERVAL: Duration = Duration::from_secs(30);
    /// Holding View this long during a hand-off returns to GameMode.
    const VIEW_HOLD: Duration = Duration::from_millis(500);
    /// Held at start to sign in on the Windows desktop for once.
    pub const DESKTOP_COMBO: u32 = B | BACK;
    /// How long a pad gets to report the start-up combination.
    const STARTUP_WINDOW: Duration = Duration::from_millis(500);
    /// View+RB held during a game takes a screenshot.
    const CAPTURE_HOLD: Duration = Duration::from_millis(400);

    const MAPPING: &[(u32, Action)] = &[
        (DPAD_UP, Action::Up),
        (DPAD_DOWN, Action::Down),
        (DPAD_LEFT, Action::Left),
        (DPAD_RIGHT, Action::Right),
        (A, Action::Confirm),
        (B, Action::Back),
        (X, Action::X),
        (Y, Action::Y),
        (START, Action::Menu),
        (BACK, Action::View),
        (LB, Action::Lb),
        (RB, Action::Rb),
        (LT, Action::Lt),
        (RT, Action::Rt),
        (GUIDE, Action::Guide),
    ];

    type GetStateFn = unsafe extern "system" fn(u32, *mut XINPUT_STATE) -> u32;

    /// `XInputGetStateEx` (ordinal 100) is the only way to read the Guide button.
    fn get_state_fn() -> GetStateFn {
        unsafe {
            if let Ok(lib) = LoadLibraryA(s!("xinput1_4.dll")) {
                if let Some(f) = GetProcAddress(lib, PCSTR(100usize as *const u8)) {
                    return std::mem::transmute::<unsafe extern "system" fn() -> isize, GetStateFn>(f);
                }
            }
        }
        fallback
    }

    unsafe extern "system" fn fallback(slot: u32, state: *mut XINPUT_STATE) -> u32 {
        unsafe { XInputGetState(slot, state) }
    }

    fn read_buttons(state: &XINPUT_STATE) -> u32 {
        let pad = &state.Gamepad;
        let mut bits = pad.wButtons.0 as u32;
        if pad.sThumbLY > STICK_THRESHOLD {
            bits |= DPAD_UP;
        }
        if pad.sThumbLY < -STICK_THRESHOLD {
            bits |= DPAD_DOWN;
        }
        if pad.sThumbLX < -STICK_THRESHOLD {
            bits |= DPAD_LEFT;
        }
        if pad.sThumbLX > STICK_THRESHOLD {
            bits |= DPAD_RIGHT;
        }
        if pad.bLeftTrigger > TRIGGER_THRESHOLD {
            bits |= LT;
        }
        if pad.bRightTrigger > TRIGGER_THRESHOLD {
            bits |= RT;
        }
        bits
    }

    /// Whether every button of `combo` is down on some pad right now (or
    /// within a short window, so a pad still enumerating gets its say).
    /// Returns at once when no pad is connected.
    pub fn combo_held_at_startup(combo: u32) -> bool {
        let get_state = get_state_fn();
        let deadline = Instant::now() + STARTUP_WINDOW;
        loop {
            let mut any = false;
            for slot in 0..4 {
                let mut state = XINPUT_STATE::default();
                if unsafe { get_state(slot, &mut state) } == 0 {
                    any = true;
                    if read_buttons(&state) & combo == combo {
                        return true;
                    }
                }
            }
            if !any || Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn start(app: AppHandle) {
        std::thread::spawn(move || {
            let get_state = get_state_fn();
            let mut connected = [false; 4];
            let mut last_probe = [None::<Instant>; 4];
            let mut repeater = Repeater::default();
            let mut combo_since: Option<Instant> = None;
            let mut combo_fired = false;
            let mut last_battery: Option<Instant> = None;
            let mut was_connected = connected;
            let mut view_since: Option<Instant> = None;
            let mut wand_since: Option<Instant> = None;
            let mut wand_fired = false;
            let mut wand_checked = Instant::now();
            let mut haptics = crate::haptics::PulseQueue::default();
            // Per slot, the state that last counted as activity.
            let mut last_motion = [None::<super::PadSnapshot>; 4];
            let mut capture_since: Option<Instant> = None;
            let mut capture_fired = false;

            loop {
                let now = Instant::now();
                let mut buttons = 0u32;
                // The physical D-pad alone, without the stick folded in.
                let mut dpad = 0u32;
                for slot in 0..4 {
                    if !connected[slot] && last_probe[slot].is_some_and(|t| now - t < PROBE_INTERVAL) {
                        continue;
                    }
                    last_probe[slot] = Some(now);
                    let mut state = XINPUT_STATE::default();
                    connected[slot] = unsafe { get_state(slot as u32, &mut state) } == 0;
                    if connected[slot] {
                        buttons |= read_buttons(&state);
                        dpad |= state.Gamepad.wButtons.0 as u32 & (DPAD_UP | DPAD_DOWN);
                        let pad = &state.Gamepad;
                        let snap = super::PadSnapshot {
                            buttons: pad.wButtons.0,
                            sticks: [pad.sThumbLX, pad.sThumbLY, pad.sThumbRX, pad.sThumbRY],
                            triggers: [pad.bLeftTrigger, pad.bRightTrigger],
                        };
                        // A pad turning on or off (auto power-off, flat battery)
                        // is not someone playing: only changes on a pad that
                        // was already there count.
                        match last_motion[slot] {
                            Some(before) if snap.moved_from(&before) => {
                                last_motion[slot] = Some(snap);
                                super::note_input();
                            }
                            Some(_) => {}
                            None => last_motion[slot] = Some(snap),
                        }
                    } else {
                        last_motion[slot] = None;
                    }
                }
                // Windows doesn't count controllers as activity: GameMode does.
                if buttons != 0 {
                    crate::idle::touch_input();
                }

                // Input goes only to the GameMode window that has focus.
                let target = if overlay::is_open() {
                    Some(overlay::LABEL)
                } else if window::is_foreground(&app) {
                    Some("main")
                } else {
                    None
                };
                // Keyboard assist ends once GameMode is back, and never runs
                // during a game (it does while one is starting: store dialogs).
                if assist::is_active() {
                    let playing = app
                        .state::<crate::AppState>()
                        .launcher
                        .current()
                        .is_some_and(|s| s.phase == crate::launcher::Phase::Running);
                    if playing || (target.is_some() && assist::age_ms() > assist::GRACE_MS) {
                        assist::disable();
                    }
                }
                let assisting = target.is_none() && assist::is_active();
                // Wand's overlay is up over the game: the controller drives it,
                // but only while Wand has the keyboard — never type into the game.
                if crate::wand::is_open() && now - wand_checked >= WAND_CHECK {
                    wand_checked = now;
                    if target.is_some() || !crate::wand::has_focus() {
                        crate::wand::forget();
                    }
                }
                let wand = target.is_none() && crate::wand::is_open();
                // The system keyboard is up over another window: it takes
                // the controller, whatever mode is on underneath (it isn't
                // a target itself: that would end assist and Wand modes).
                // GameMode in front has its own in-page keyboard instead.
                if keyboard::is_open() && target.is_some() {
                    keyboard::hide(&app);
                }
                let typing = target.is_none() && keyboard::is_open();
                for (bit, repeat) in repeater.step(buttons, DIRECTIONS, now) {
                    let Some(&(_, action)) = MAPPING.iter().find(|(b, _)| *b == bit) else { continue };
                    // View held + D-pad up/down during a game: volume, with
                    // the on-screen indicator as feedback. Only the real
                    // D-pad: View + left stick is common in games (scoreboard).
                    if target.is_none() && buttons & BACK != 0 && (bit == DPAD_UP || bit == DPAD_DOWN) && dpad & bit != 0 && crate::osd::combo_active(&app) {
                        crate::osd::nudge(&app, if bit == DPAD_UP { crate::osd::PAD_STEP } else { -crate::osd::PAD_STEP });
                        continue;
                    }
                    // Guide closes the keyboard and keeps its meaning
                    // underneath: leaving Wand, or summoning GameMode.
                    if typing && action != Action::Guide {
                        let _ = app.emit_to(keyboard::LABEL, "gamepad://nav", NavEvent { action, repeat });
                    } else if wand {
                        if typing {
                            keyboard::hide(&app);
                        }
                        wand_key(&app, action);
                    } else if action == Action::Guide {
                        summon(&app, target, true);
                    } else if let Some(label) = target {
                        let _ = app.emit_to(label, "gamepad://nav", NavEvent { action, repeat });
                    } else if assisting && assist::types_keys() {
                        assist_key(&app, action);
                    }
                }

                // Holding View: the way back from any hand-off, also from apps
                // that use the controller themselves (Xbox app, Microsoft Store).
                if assisting && buttons & BACK != 0 {
                    let since = *view_since.get_or_insert(now);
                    if now - since >= VIEW_HOLD {
                        view_since = None;
                        crate::storefront::return_to_gamemode(&app);
                    }
                } else {
                    view_since = None;
                }
                let foreground = target.is_some();

                if connected != was_connected || last_battery.is_none_or(|t| now - t >= BATTERY_INTERVAL) {
                    was_connected = connected;
                    last_battery = Some(now);
                    update_controllers(&app, &connected);
                }

                // Back+Start held: fallback for when Game Bar swallows Guide.
                if buttons & (BACK | START) == BACK | START {
                    let since = *combo_since.get_or_insert(now);
                    let in_game = app
                        .state::<crate::AppState>()
                        .launcher
                        .current()
                        .is_some_and(|s| s.phase == crate::launcher::Phase::Running);
                    let hold = if in_game { COMBO_HOLD_IN_GAME } else { COMBO_HOLD };
                    let overlay_target = target.filter(|t| *t == overlay::LABEL);
                    if !combo_fired && now - since >= hold && (!foreground || overlay_target.is_some()) {
                        combo_fired = true;
                        summon(&app, overlay_target, false);
                    }
                } else {
                    combo_since = None;
                    combo_fired = false;
                }

                // LB+RB held during a game: Wand's overlay, when switched on.
                if target.is_none() && buttons & (LB | RB) == LB | RB {
                    let since = *wand_since.get_or_insert(now);
                    if !wand_fired && now - since >= WAND_HOLD {
                        wand_fired = true;
                        let state = app.state::<crate::AppState>();
                        let playing = state.launcher.current().is_some_and(|s| s.phase == crate::launcher::Phase::Running);
                        if playing && state.db.lock().settings.wand_combo {
                            if crate::wand::is_open() {
                                crate::wand::close(&app);
                            } else if crate::wand::installed() {
                                crate::wand::open(&app);
                            }
                        }
                    }
                } else {
                    wand_since = None;
                    wand_fired = false;
                }

                // Interface vibration: queued patterns become motor speeds here,
                // on the same thread that knows which slots are connected.
                for (pattern, level) in crate::haptics::take_pending() {
                    haptics.push(pattern, level, now);
                }
                if let Some((strong, weak)) = haptics.step(now) {
                    let vibration = XINPUT_VIBRATION { wLeftMotorSpeed: strong, wRightMotorSpeed: weak };
                    for slot in (0..4u32).filter(|&s| connected[s as usize]) {
                        unsafe { XInputSetState(slot, &vibration) };
                    }
                }

                // View+RB held during a game: a screenshot (not with Start,
                // which is the Back+Start combo above).
                if target.is_none() && buttons & (BACK | RB | START) == BACK | RB {
                    let since = *capture_since.get_or_insert(now);
                    if !capture_fired && now - since >= CAPTURE_HOLD {
                        capture_fired = true;
                        let state = app.state::<crate::AppState>();
                        let playing = state.launcher.current().is_some_and(|s| s.phase == crate::launcher::Phase::Running);
                        if playing && state.db.lock().settings.capture_combo {
                            crate::capture::take_async(app.clone());
                        }
                    }
                } else {
                    capture_since = None;
                    capture_fired = false;
                }

                std::thread::sleep(POLL);
            }
        });
    }

    fn update_controllers(app: &AppHandle, connected: &[bool; 4]) {
        let list: Vec<Controller> = (0..4u32)
            .filter(|&slot| connected[slot as usize])
            .map(|slot| {
                let mut info = XINPUT_BATTERY_INFORMATION::default();
                let ok = unsafe { XInputGetBatteryInformation(slot, BATTERY_DEVTYPE_GAMEPAD, &mut info) } == 0;
                let wired = !ok || info.BatteryType == BATTERY_TYPE_WIRED || info.BatteryType == BATTERY_TYPE_DISCONNECTED;
                Controller { slot, wired, level: info.BatteryLevel.0 }
            })
            .collect();
        let mut current = CONTROLLERS.lock().unwrap();
        if *current != list {
            *current = list.clone();
            let _ = app.emit("gamepad://controllers", &list);
        }
    }

    /// Drives Wand (overlay or app): A = Enter, D-pad = arrows,
    /// LB/RB = Shift+Tab/Tab, X = Space, Y = on-screen keyboard, B = back;
    /// View or Guide leave.
    fn wand_key(app: &AppHandle, action: Action) {
        let key = match action {
            Action::Back => {
                crate::wand::back(app);
                return;
            }
            Action::View | Action::Guide => {
                crate::wand::leave(app);
                return;
            }
            Action::Confirm => Key::Enter,
            Action::Up => Key::Up,
            Action::Down => Key::Down,
            Action::Left => Key::Left,
            Action::Right => Key::Right,
            Action::Rb => Key::Tab,
            Action::Lb => Key::ShiftTab,
            Action::X => Key::Space,
            Action::Y => {
                keyboard::show(app);
                return;
            }
            _ => return,
        };
        assist::press(key);
    }

    /// Drives a store dialog with the keys it understands; Y opens the
    /// on-screen keyboard, View goes back.
    fn assist_key(app: &AppHandle, action: Action) {
        let key = match action {
            Action::Confirm => Key::Enter,
            Action::Back => Key::Escape,
            Action::Up => Key::Up,
            Action::Down => Key::Down,
            Action::Left => Key::Left,
            Action::Right => Key::Right,
            Action::Rb => Key::Tab,
            Action::Lb => Key::ShiftTab,
            Action::X => Key::Space,
            Action::View => {
                crate::storefront::return_to_gamemode(app);
                return;
            }
            Action::Y => {
                keyboard::show(app);
                return;
            }
            _ => return,
        };
        assist::press(key);
    }

    /// Guide: toggles the overlay over a running game, otherwise brings
    /// GameMode back and opens its quick menu.
    fn summon(app: &AppHandle, target: Option<&str>, from_guide: bool) {
        // Whatever comes up next, the keyboard goes.
        keyboard::hide(app);
        let guide = NavEvent { action: Action::Guide, repeat: false };
        if let Some(label) = target {
            let _ = app.emit_to(label, "gamepad://nav", guide);
            return;
        }
        let state = app.state::<crate::AppState>();
        let session = state.launcher.current();
        let playing = session.as_ref().is_some_and(|s| s.phase == crate::launcher::Phase::Running);
        // Steam reacts to Guide in its games: when the user leaves Guide to
        // Steam's overlay, only Back+Start brings GameMode's up.
        let steam_game = session.as_ref().is_some_and(|s| s.game_id.starts_with("steam:"));
        if from_guide && playing && steam_game && state.db.lock().settings.steam_guide {
            return;
        }
        if playing && state.db.lock().settings.overlay_enabled && overlay::window(app).is_some() {
            // The only vibration during a game: confirms the press that
            // opens the overlay, before the half-second it takes to appear.
            crate::haptics::rumble(crate::haptics::Pattern::Tick, state.db.lock().settings.haptics);
            overlay::show_after_guide(app);
            return;
        }
        window::bring_to_front(app);
        let _ = app.emit_to("main", "gamepad://nav", guide);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn stick_drift_is_not_activity() {
        let pad = |buttons, sticks, triggers| PadSnapshot { buttons, sticks, triggers };
        let rest = pad(0, [0, 0, 0, 0], [0, 0]);
        assert!(!pad(0, [900, -700, 300, 0], [10, 0]).moved_from(&rest));
        // A worn stick resting anywhere, with noise around it.
        let worn = pad(0, [2_040, 0, 0, 0], [0, 0]);
        assert!(!pad(0, [2_070, 0, 0, 0], [0, 0]).moved_from(&worn));
        assert!(!pad(0, [2_010, 30, -30, 0], [0, 0]).moved_from(&worn));
        // Real input.
        assert!(pad(0, [20_000, 0, 0, 0], [0, 0]).moved_from(&rest));
        assert!(pad(0, [0, 0, 0, 0], [200, 0]).moved_from(&rest));
        assert!(pad(0x1000, [0, 0, 0, 0], [0, 0]).moved_from(&rest));
    }

    #[test]
    fn repeats_only_repeatable_buttons_after_delay() {
        let mut r = Repeater::default();
        let t0 = Instant::now();
        assert_eq!(r.step(0b11, 0b01, t0), vec![(0b01, false), (0b10, false)]);
        assert!(r.step(0b11, 0b01, t0 + Duration::from_millis(200)).is_empty());
        assert_eq!(r.step(0b11, 0b01, t0 + REPEAT_DELAY), vec![(0b01, true)]);
        assert!(r.step(0b11, 0b01, t0 + REPEAT_DELAY + Duration::from_millis(10)).is_empty());
        assert_eq!(r.step(0b11, 0b01, t0 + REPEAT_DELAY + REPEAT_RATE), vec![(0b01, true)]);
        assert!(r.step(0, 0b01, t0 + Duration::from_secs(1)).is_empty());
        assert_eq!(r.step(0b01, 0b01, t0 + Duration::from_secs(2)), vec![(0b01, false)]);
    }
}
