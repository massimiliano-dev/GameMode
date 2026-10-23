//! Hand-offs to store windows GameMode opens on purpose (install and
//! uninstall dialogs, the Xbox app…). During a hand-off, holding View brings
//! GameMode back. For windows that don't understand a controller the
//! controller also types keys: A = Enter, B = Esc, D-pad = arrows,
//! LB/RB = Shift+Tab/Tab, X = Space, Y = on-screen keyboard (`keyboard`),
//! and a short View press goes back.
//! Never active while a game runs, so keys are never typed into a game.

// Driven from the XInput thread, which only exists on Windows.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// Unix milliseconds the mode was switched on, 0 when off.
static ENABLED_AT: AtomicI64 = AtomicI64::new(0);
/// Whether the controller types keys, or only offers the way back.
static KEYS: AtomicBool = AtomicBool::new(false);
/// Turns itself off after this long, whatever happens.
const MAX_MS: i64 = 15 * 60 * 1000;
/// Right after a hand-off GameMode may still have focus for a moment.
pub const GRACE_MS: i64 = 3000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Starts a hand-off. `keys`: the window doesn't handle a controller itself.
pub fn enable(keys: bool) {
    KEYS.store(keys, Ordering::SeqCst);
    ENABLED_AT.store(now_ms(), Ordering::SeqCst);
}

pub fn types_keys() -> bool {
    KEYS.load(Ordering::SeqCst)
}

pub fn disable() {
    ENABLED_AT.store(0, Ordering::SeqCst);
}

pub fn is_active() -> bool {
    let at = ENABLED_AT.load(Ordering::SeqCst);
    at > 0 && now_ms() - at < MAX_MS
}

/// Milliseconds since the mode was switched on.
pub fn age_ms() -> i64 {
    now_ms() - ENABLED_AT.load(Ordering::SeqCst)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key {
    Enter,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Tab,
    ShiftTab,
    Space,
    Backspace,
    /// Wand's overlay shortcuts.
    AltW,
    AltBacktick,
}

/// Types `key` into whatever window has focus.
#[cfg(windows)]
pub fn press(key: Key) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_BACK,
        VK_DOWN, VK_ESCAPE, VK_LEFT, VK_MENU, VK_OEM_3, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP, VK_W,
    };

    fn input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: 0,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    let vk = match key {
        Key::Enter => VK_RETURN,
        Key::Escape => VK_ESCAPE,
        Key::Up => VK_UP,
        Key::Down => VK_DOWN,
        Key::Left => VK_LEFT,
        Key::Right => VK_RIGHT,
        Key::Tab | Key::ShiftTab => VK_TAB,
        Key::Space => VK_SPACE,
        Key::Backspace => VK_BACK,
        Key::AltW => VK_W,
        Key::AltBacktick => VK_OEM_3,
    };
    let modifier = match key {
        Key::ShiftTab => Some(VK_SHIFT),
        Key::AltW | Key::AltBacktick => Some(VK_MENU),
        _ => None,
    };
    let mut inputs = Vec::with_capacity(4);
    if let Some(m) = modifier {
        inputs.push(input(m, false));
    }
    inputs.push(input(vk, false));
    inputs.push(input(vk, true));
    if let Some(m) = modifier {
        inputs.push(input(m, true));
    }
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(not(windows))]
pub fn press(key: Key) {
    let _ = key;
}

/// Types `text` into whatever window has focus, as Unicode characters
/// (VK_PACKET): text fields and chat boxes take them like keystrokes, which
/// is what the on-screen keyboard needs. Games polling key states don't see
/// letters this way, and some anti-cheats drop injected input: both are
/// out of reach without a keyboard driver.
#[cfg(windows)]
pub fn type_text(text: &str) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, VIRTUAL_KEY,
    };

    let inputs: Vec<INPUT> = text
        .encode_utf16()
        .flat_map(|unit| {
            [false, true].map(|up| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: VIRTUAL_KEY(0),
                        wScan: unit,
                        dwFlags: if up { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
        })
        .collect();
    if inputs.is_empty() {
        return;
    }
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(not(windows))]
pub fn type_text(text: &str) {
    let _ = text;
}
