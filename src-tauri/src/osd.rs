//! A small "screenshot saved" card over the game. Unlike the overlay, its
//! window never takes focus (the game keeps the controller and the screen),
//! ignores the mouse and goes away by itself. Created on first use, so a
//! session without screenshots pays nothing for it.

// Shown from the XInput thread and the capture command, Windows only.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::capture::Capture;

pub const LABEL: &str = "osd";
/// How long the card stays up.
const SHOW_FOR: Duration = Duration::from_millis(2800);
/// Logical size of the card.
const WIDTH: f64 = 460.0;
const HEIGHT: f64 = 124.0;
const MARGIN: f64 = 28.0;
/// Bumped on every show and hide, so only the newest hide timer acts.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// What the card shows; the page asks for it when it loads, since the first
/// event can arrive before the window is ready to listen.
static CURRENT: Mutex<Option<Notice>> = Mutex::new(None);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Notice {
    Saved { capture: Capture },
    Error { message: String },
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// The notice on screen, for the page when it loads.
pub fn current() -> Option<Notice> {
    CURRENT.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn show_saved(app: &AppHandle, capture: &Capture, game_window: Option<isize>) {
    show(app, Notice::Saved { capture: capture.clone() }, game_window);
}

pub fn show_error(app: &AppHandle, message: &str) {
    show(app, Notice::Error { message: message.to_string() }, None);
}

fn show(app: &AppHandle, notice: Notice, game_window: Option<isize>) {
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(notice.clone());
    #[cfg(windows)]
    {
        use tauri::Emitter;
        let Some(w) = window(app).or_else(|| imp::create(app)) else { return };
        imp::place(&w, game_window);
        let _ = app.emit_to(LABEL, "osd://show", &notice);
        imp::show(&w);
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SHOW_FOR);
            if GENERATION.load(Ordering::SeqCst) == generation {
                hide(&app);
            }
        });
    }
    #[cfg(not(windows))]
    {
        let _ = (app, notice, game_window);
    }
}

pub fn hide(app: &AppHandle) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    *CURRENT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    if let Some(w) = window(app) {
        #[cfg(windows)]
        imp::hide(&w);
        #[cfg(not(windows))]
        let _ = w.hide();
    }
}

#[cfg(windows)]
mod imp {
    use tauri::{AppHandle, LogicalSize, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
    use windows::Win32::Foundation::HWND;

    use super::{HEIGHT, LABEL, MARGIN, WIDTH};
    use crate::capture::imp::{monitor_of, monitor_rect};

    /// The hidden card window: transparent, topmost, never activated.
    pub fn create(app: &AppHandle) -> Option<WebviewWindow> {
        let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html?osd=1".into()))
            .title("GameMode")
            .transparent(true)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            .focused(false)
            .focusable(false)
            .visible(false)
            .inner_size(WIDTH, HEIGHT)
            .build();
        match built {
            Ok(w) => {
                let _ = w.set_ignore_cursor_events(true);
                Some(w)
            }
            Err(err) => {
                eprintln!("osd window: {err}");
                None
            }
        }
    }

    /// Shown without activation. Not `w.show()`: tao clears the "don't
    /// focus" marker once the window exists and then shows with `SW_SHOW`,
    /// which would take the foreground from the game.
    pub fn show(w: &WebviewWindow) {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE};
        let Ok(h) = w.hwnd() else { return };
        unsafe {
            let _ = ShowWindow(HWND(h.0), SW_SHOWNOACTIVATE);
        }
        crate::window::raise_topmost_raw(h.0 as isize);
    }

    /// The counterpart of `show`: tao never saw the window as visible, so
    /// its own `hide()` would do nothing.
    pub fn hide(w: &WebviewWindow) {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        if let Ok(h) = w.hwnd() {
            unsafe {
                let _ = ShowWindow(HWND(h.0), SW_HIDE);
            }
        }
    }

    /// Bottom-right corner of the screen the game is on.
    pub fn place(w: &WebviewWindow, game_window: Option<isize>) {
        let Some((left, top, width, height)) = monitor_rect(monitor_of(game_window)) else { return };
        // Move first: the logical size depends on that monitor's scale.
        let _ = w.set_position(PhysicalPosition::new(left, top));
        let _ = w.set_size(LogicalSize::new(WIDTH, HEIGHT));
        let scale = w.scale_factor().unwrap_or(1.0);
        let size = w
            .outer_size()
            .unwrap_or_else(|_| PhysicalSize::new((WIDTH * scale) as u32, (HEIGHT * scale) as u32));
        let margin = (MARGIN * scale) as i32;
        let x = left + width - size.width as i32 - margin;
        let y = top + height - size.height as i32 - margin;
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
}
