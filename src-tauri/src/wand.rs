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
    let app = app.clone();
    std::thread::spawn(move || {
        if crate::window::windows_of(PROCESSES).is_empty() {
            let _ = std::process::Command::new(exe).spawn();
        }
        // Wand takes a few seconds to start; push its window up once it's there.
        for _ in 0..40 {
            if has_focus() {
                if let Some(&hwnd) = crate::window::windows_of(PROCESSES).first() {
                    crate::window::move_to_gamemode_screen(&app, hwnd, true);
                }
                entered(&app, &IN_APP);
                return;
            }
            if let Some(&hwnd) = crate::window::windows_of(PROCESSES).first() {
                crate::window::move_to_gamemode_screen(&app, hwnd, true);
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
                entered(&app, &OPEN);
                return;
            }
        }
    });
}

pub fn close(app: &AppHandle) {
    if OPEN.swap(false, Ordering::SeqCst) {
        // Thawed first: a frozen game can't take the shortcut.
        pause::resume();
        assist::press(hotkey(app));
    }
}

/// The game ended, GameMode came back or the game took the focus again:
/// Wand is out of the picture.
pub fn forget() {
    OPEN.store(false, Ordering::SeqCst);
    IN_APP.store(false, Ordering::SeqCst);
    pause::resume();
}

/// Wand has the keyboard: the controller drives it, and (when switched on)
/// the game is frozen meanwhile — games read the controller even without
/// focus, so the character would walk around while picking mods.
fn entered(app: &AppHandle, flag: &AtomicBool) {
    flag.store(true, Ordering::SeqCst);
    let state = app.state::<AppState>();
    if state.db.lock().settings.wand_pause {
        pause::suspend(&state.launcher.running_pids());
    }
}

/// Freezing and thawing a game's processes (what Windows does to suspended
/// apps). Always undone when Wand is left; elevated games can't be touched.
mod pause {
    static SUSPENDED: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

    #[cfg(windows)]
    fn call(name: windows::core::PCSTR, pid: u32) -> bool {
        use windows::core::s;
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_SUSPEND_RESUME};
        type NtProcessFn = unsafe extern "system" fn(HANDLE) -> i32;
        unsafe {
            let Ok(ntdll) = GetModuleHandleA(s!("ntdll.dll")) else { return false };
            let Some(f) = GetProcAddress(ntdll, name) else { return false };
            let f = std::mem::transmute::<unsafe extern "system" fn() -> isize, NtProcessFn>(f);
            let Ok(handle) = OpenProcess(PROCESS_SUSPEND_RESUME, false, pid) else { return false };
            let ok = f(handle) >= 0;
            let _ = CloseHandle(handle);
            ok
        }
    }

    pub fn suspend(pids: &[u32]) {
        let mut suspended = SUSPENDED.lock().unwrap();
        for &pid in pids {
            #[cfg(windows)]
            if !suspended.contains(&pid) && call(windows::core::s!("NtSuspendProcess"), pid) {
                suspended.push(pid);
            }
            #[cfg(not(windows))]
            let _ = (pid, &mut suspended);
        }
    }

    pub fn resume() {
        for pid in std::mem::take(&mut *SUSPENDED.lock().unwrap()) {
            #[cfg(windows)]
            call(windows::core::s!("NtResumeProcess"), pid);
            #[cfg(not(windows))]
            let _ = pid;
        }
    }
}
