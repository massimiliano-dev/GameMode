//! In-game overlay: a transparent always-on-top window that Guide opens over a
//! running game (clock, battery, volume, back to GameMode, close game). It
//! covers borderless and windowed games; exclusive-fullscreen games drop to
//! the desktop when another window takes focus, which no overlay without
//! code injection can avoid.

// Opened from the XInput thread, which only exists on Windows.
#![cfg_attr(not(windows), allow(dead_code))]

use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub const LABEL: &str = "overlay";

/// Tracked here so the 8 ms controller loop doesn't query the window.
static VISIBLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Bumped on every show, so only the newest keep-on-top loop runs.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// How long the overlay keeps trying to take focus back after opening.
const FOCUS_FIGHT: std::time::Duration = std::time::Duration::from_secs(3);

/// While the overlay is up it gets all controller input, even when Game Bar
/// or the game itself grabbed the foreground for a moment.
pub fn is_open() -> bool {
    VISIBLE.load(std::sync::atomic::Ordering::SeqCst)
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// Creates the hidden overlay window. Windows only: elsewhere Guide just
/// brings the launcher back.
pub fn create(app: &AppHandle) {
    #[cfg(windows)]
    {
        use tauri::{WebviewUrl, WebviewWindowBuilder};
        let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html?overlay=1".into()))
            .title("GameMode")
            .transparent(true)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            .focused(false)
            .visible(false)
            .build();
        if let Err(err) = built {
            eprintln!("overlay window: {err}");
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

pub fn is_foreground(app: &AppHandle) -> bool {
    let Some(w) = window(app) else { return false };
    #[cfg(windows)]
    {
        w.hwnd().map(|h| crate::window::is_hwnd_foreground(h.0 as isize)).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        w.is_focused().unwrap_or(false)
    }
}

/// Shows the overlay over the screen the game is on.
pub fn show(app: &AppHandle) {
    let Some(w) = window(app) else { return };
    if let Ok(Some(monitor)) = w.primary_monitor() {
        let _ = w.set_position(*monitor.position());
        let _ = w.set_size(*monitor.size());
    }
    VISIBLE.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = w.show();
    let _ = w.set_always_on_top(true);
    claim_focus(&w);
    let _ = app.emit_to(LABEL, "overlay://open", ());
    // Guide also wakes Steam and Xbox Game Bar, which jump on top right
    // after us. While open, keep the overlay above them (input reaches it
    // either way) and, for the first seconds, take focus back too.
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        while is_open() && GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if !is_open() || is_foreground(&app) {
                continue;
            }
            let Some(w) = window(&app) else { break };
            #[cfg(windows)]
            if let Ok(hwnd) = w.hwnd() {
                crate::window::raise_topmost_raw(hwnd.0 as isize);
            }
            if started.elapsed() < FOCUS_FIGHT {
                claim_focus(&w);
            }
        }
    });
}

fn claim_focus(w: &WebviewWindow) {
    let _ = w.set_focus();
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        crate::window::force_foreground_raw(hwnd.0 as isize);
    }
}

pub fn hide(app: &AppHandle) {
    VISIBLE.store(false, std::sync::atomic::Ordering::SeqCst);
    if let Some(w) = window(app) {
        let _ = w.hide();
    }
}
