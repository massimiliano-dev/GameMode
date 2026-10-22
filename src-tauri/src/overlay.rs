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
    // Guide also wakes Xbox Game Bar and some games take focus back on their
    // next frame: try again once they've had their turn.
    let app = app.clone();
    std::thread::spawn(move || {
        for delay in [150, 450, 1000] {
            std::thread::sleep(std::time::Duration::from_millis(delay));
            if !is_open() || is_foreground(&app) {
                return;
            }
            if let Some(w) = window(&app) {
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
