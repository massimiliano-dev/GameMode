//! In-game overlay: a transparent always-on-top window that Guide opens over a
//! running game (clock, battery, volume, back to GameMode, close game). It
//! covers borderless and windowed games; exclusive-fullscreen games drop to
//! the desktop when another window takes focus, which no overlay without
//! code injection can avoid.

// Opened from the XInput thread, which only exists on Windows.
#![cfg_attr(not(windows), allow(dead_code))]

use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

pub const LABEL: &str = "overlay";

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
    let _ = w.show();
    let _ = w.set_always_on_top(true);
    let _ = w.set_focus();
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        crate::window::force_foreground_raw(hwnd.0 as isize);
    }
    let _ = app.emit_to(LABEL, "overlay://open", ());
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = window(app) {
        let _ = w.hide();
    }
}
