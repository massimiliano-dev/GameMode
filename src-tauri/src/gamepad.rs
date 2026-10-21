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

    use tauri::{AppHandle, Emitter};
    use windows::core::{s, PCSTR};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
    use windows::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};

    use super::{Action, NavEvent, Repeater};
    use crate::window;

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
    const POLL: Duration = Duration::from_millis(8);
    /// Polling an empty slot is slow, so disconnected slots are probed rarely.
    const PROBE_INTERVAL: Duration = Duration::from_secs(1);

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

    pub fn start(app: AppHandle) {
        std::thread::spawn(move || {
            let get_state = get_state_fn();
            let mut connected = [false; 4];
            let mut last_probe = [None::<Instant>; 4];
            let mut repeater = Repeater::default();
            let mut combo_since: Option<Instant> = None;
            let mut combo_fired = false;

            loop {
                let now = Instant::now();
                let mut buttons = 0u32;
                for slot in 0..4 {
                    if !connected[slot] && last_probe[slot].is_some_and(|t| now - t < PROBE_INTERVAL) {
                        continue;
                    }
                    last_probe[slot] = Some(now);
                    let mut state = XINPUT_STATE::default();
                    connected[slot] = unsafe { get_state(slot as u32, &mut state) } == 0;
                    if connected[slot] {
                        buttons |= read_buttons(&state);
                    }
                }

                let foreground = window::is_foreground(&app);
                for (bit, repeat) in repeater.step(buttons, DIRECTIONS, now) {
                    let Some(&(_, action)) = MAPPING.iter().find(|(b, _)| *b == bit) else { continue };
                    if action == Action::Guide {
                        summon(&app, foreground);
                    } else if foreground {
                        let _ = app.emit("gamepad://nav", NavEvent { action, repeat });
                    }
                }

                // Back+Start held: fallback for when Game Bar swallows Guide.
                if buttons & (BACK | START) == BACK | START {
                    let since = *combo_since.get_or_insert(now);
                    if !combo_fired && now - since >= COMBO_HOLD && !foreground {
                        combo_fired = true;
                        summon(&app, false);
                    }
                } else {
                    combo_since = None;
                    combo_fired = false;
                }

                std::thread::sleep(POLL);
            }
        });
    }

    fn summon(app: &AppHandle, foreground: bool) {
        if !foreground {
            window::bring_to_front(app);
        }
        let _ = app.emit("gamepad://nav", NavEvent { action: Action::Guide, repeat: false });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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
