//! Wand (formerly WeMod) mods from the controller. Wand has no API, but its
//! in-game overlay opens and closes with a keyboard shortcut (Alt+W, or
//! Alt+` in its settings): GameMode presses it for the user and, while
//! Wand's overlay is up, the controller types the keys that drive it.

// The controller side runs on the XInput thread, which only exists on Windows.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};

use crate::assist::{self, Key};
use crate::AppState;

/// Wand's overlay is open over the game, in a window of its own that has
/// the keyboard. Only then does the controller type keys: sent to the game
/// instead, they'd flip it between controller and keyboard controls.
static OPEN: AtomicBool = AtomicBool::new(false);
/// The Wand app itself is in front (picking a game, pressing Play).
static IN_APP: AtomicBool = AtomicBool::new(false);

const PROCESSES: &[&str] = &["wand.exe", "wemod.exe"];

/// A Wand window is in front.
pub fn has_focus() -> bool {
    #[cfg(windows)]
    {
        crate::window::foreground_process_name().is_some_and(|n| PROCESSES.contains(&n.as_str()))
    }
    #[cfg(not(windows))]
    {
        let _ = PROCESSES;
        false
    }
}

/// Where Wand (and WeMod before the rename) installs itself.
fn candidates() -> Vec<PathBuf> {
    let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return Vec::new() };
    vec![local.join("Wand").join("Wand.exe"), local.join("WeMod").join("WeMod.exe")]
}

pub fn installed() -> bool {
    candidates().iter().any(|p| p.is_file())
}

pub fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst) || IN_APP.load(Ordering::SeqCst)
}

/// Opens the Wand app (or brings it in front), where mods are attached to
/// a game: pick it and press Play, which also hooks into a game already
/// running. The controller drives the app meanwhile.
pub fn open_app(app: &AppHandle) {
    crate::overlay::hide(app);
    let Some(exe) = candidates().into_iter().find(|p| p.is_file()) else { return };
    std::thread::spawn(move || {
        if crate::window::windows_of(PROCESSES).is_empty() {
            let _ = std::process::Command::new(exe).spawn();
        }
        // Wand takes a few seconds to start; push its window up once it's there.
        for _ in 0..40 {
            if has_focus() {
                IN_APP.store(true, Ordering::SeqCst);
                return;
            }
            if let Some(&hwnd) = crate::window::windows_of(PROCESSES).first() {
                crate::window::force_foreground_raw(hwnd);
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
}

/// B: Esc in the app, closes the overlay.
pub fn back(app: &AppHandle) {
    if IN_APP.load(Ordering::SeqCst) {
        assist::press(Key::Escape);
    } else {
        close(app);
    }
}

/// View or Guide: out of Wand, back to the game (or GameMode).
pub fn leave(app: &AppHandle) {
    close(app);
    forget();
    let pids = app.state::<AppState>().launcher.running_pids();
    if !crate::window::resume_game(app, &pids) {
        crate::window::bring_to_front(app);
    }
}

fn hotkey(app: &AppHandle) -> Key {
    if app.state::<AppState>().db.lock().settings.wand_backtick {
        Key::AltBacktick
    } else {
        Key::AltW
    }
}

/// From GameMode's overlay (or the shortcut): back to the game, then
/// Wand's overlay on top of it.
pub fn open(app: &AppHandle) {
    crate::overlay::hide(app);
    let pids = app.state::<AppState>().launcher.running_pids();
    crate::window::resume_game(app, &pids);
    let app = app.clone();
    std::thread::spawn(move || {
        // Let the game take the keyboard first: Wand reads the shortcut there.
        std::thread::sleep(std::time::Duration::from_millis(400));
        assist::press(hotkey(&app));
        // Controller help only if Wand's overlay came up as its own window;
        // drawn inside the game it handles input itself (or takes the mouse).
        for _ in 0..10 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if has_focus() {
                OPEN.store(true, Ordering::SeqCst);
                return;
            }
        }
    });
}

pub fn close(app: &AppHandle) {
    if OPEN.swap(false, Ordering::SeqCst) {
        assist::press(hotkey(app));
    }
}

/// The game ended or GameMode came back: Wand's overlay went with it.
pub fn forget() {
    OPEN.store(false, Ordering::SeqCst);
    IN_APP.store(false, Ordering::SeqCst);
}
