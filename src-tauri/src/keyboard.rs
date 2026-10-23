//! System on-screen keyboard: a window of GameMode's own that never takes
//! focus (WS_EX_NOACTIVATE), so what it types with SendInput lands in the
//! window that has the keyboard — a store dialog, a game's chat, Wand's
//! search box. Windows' own TabTip is no option: the shell starts it, which
//! console mode lacks, and it ignores the controller anyway.
//! Opened by Y during a hand-off, from the in-game overlay, or by itself
//! when a text field takes focus in a store window (see `uia`). Driven from
//! the XInput thread while open. Exclusive-fullscreen games cover it like
//! any other topmost window, the overlay's limit too.

// Opened from the XInput thread, which only exists on Windows.
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::assist::{self, Key};
use crate::uia::{self, TextField};

pub const LABEL: &str = "keyboard";

/// Tracked here so the 8 ms controller loop doesn't query the window.
static VISIBLE: AtomicBool = AtomicBool::new(false);
/// Opened by the text-field watcher: closes again when the field loses focus.
static AUTO: AtomicBool = AtomicBool::new(false);
/// The user closed the keyboard (B, Enter): the watcher doesn't open it
/// again on the same field.
static DISMISSED: AtomicBool = AtomicBool::new(false);

/// Share of the screen height the window takes, at the bottom (or the top,
/// when the field being typed into sits there).
const HEIGHT_SHARE: f64 = 0.5;
/// How often the watcher asks which element has the keyboard focus.
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(300);
/// Consecutive checks without a focused field before an auto-opened
/// keyboard closes: focus flickers while a page re-renders.
const CLOSE_MISSES: u32 = 2;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct OpenEvent {
    /// The field is a password: the preview shows dots.
    masked: bool,
}

/// Where the keyboard goes on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    Bottom,
    Top,
}

/// Keeps the keyboard off the field it types into: a field in the lower
/// part of the screen pushes it to the top. Coordinates are physical pixels
/// of the monitor (`top`, `height`) and the field's bottom edge.
pub fn placement_for(monitor_top: i32, monitor_height: u32, field_bottom: Option<i32>) -> Placement {
    let keyboard_top = monitor_top + (monitor_height as f64 * (1.0 - HEIGHT_SHARE)) as i32;
    match field_bottom {
        Some(bottom) if bottom > keyboard_top => Placement::Top,
        _ => Placement::Bottom,
    }
}

/// The key a name from the frontend stands for.
pub fn key_named(name: &str) -> Option<Key> {
    Some(match name {
        "enter" => Key::Enter,
        "backspace" => Key::Backspace,
        "escape" => Key::Escape,
        "tab" => Key::Tab,
        "space" => Key::Space,
        _ => return None,
    })
}

/// While the keyboard is up it gets all controller input.
pub fn is_open() -> bool {
    VISIBLE.load(Ordering::SeqCst)
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// Creates the hidden keyboard window. Windows only: elsewhere the launcher
/// has its own in-page keyboard and nothing else to type into.
pub fn create(app: &AppHandle) {
    #[cfg(windows)]
    {
        use tauri::{WebviewUrl, WebviewWindowBuilder};
        let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html?keyboard=1".into()))
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
        match built {
            Ok(w) => {
                if let Ok(hwnd) = w.hwnd() {
                    imp::never_activate(hwnd.0 as isize);
                }
            }
            Err(err) => eprintln!("keyboard window: {err}"),
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

/// Shows the keyboard over whatever has the focus, without taking it: at
/// the bottom of the main screen, or at the top when the focused field is
/// low; a password field is shown as dots. UI Automation is asked on a
/// thread of its own, as callers include the controller loop.
pub fn show(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let field = uia::focused_text_field();
        show_placed(&app, placement_of(&app, field), field.is_some_and(|f| f.password));
    });
}

fn placement_of(app: &AppHandle, field: Option<TextField>) -> Placement {
    window(app)
        .and_then(|w| w.primary_monitor().ok().flatten())
        .map(|m| placement_for(m.position().y, m.size().height, field.map(|f| f.rect[3])))
        .unwrap_or(Placement::Bottom)
}

fn show_placed(app: &AppHandle, placement: Placement, masked: bool) {
    let Some(w) = window(app) else { return };
    if let Ok(Some(monitor)) = w.primary_monitor() {
        let pos = monitor.position();
        let size = monitor.size();
        let height = (size.height as f64 * HEIGHT_SHARE) as u32;
        let y = match placement {
            Placement::Top => pos.y,
            Placement::Bottom => pos.y + (size.height - height) as i32,
        };
        let _ = w.set_size(PhysicalSize::new(size.width, height));
        let _ = w.set_position(PhysicalPosition::new(pos.x, y));
    }
    VISIBLE.store(true, Ordering::SeqCst);
    #[cfg(windows)]
    {
        // Not `show()`: shown with SWP_NOACTIVATE the window stays inactive,
        // so the focused window keeps the keyboard.
        if let Ok(hwnd) = w.hwnd() {
            crate::window::raise_topmost_raw(hwnd.0 as isize);
        }
    }
    #[cfg(not(windows))]
    let _ = w.show();
    let _ = app.emit_to(LABEL, "keyboard://open", OpenEvent { masked });
}

/// Closed by the user: stays closed on the field that has the focus.
pub fn dismiss(app: &AppHandle) {
    DISMISSED.store(true, Ordering::SeqCst);
    hide(app);
}

pub fn hide(app: &AppHandle) {
    VISIBLE.store(false, Ordering::SeqCst);
    AUTO.store(false, Ordering::SeqCst);
    if let Some(w) = window(app) {
        let _ = w.hide();
    }
}

/// From the in-game overlay: the game takes the screen and the keyboard
/// back, then the keyboard comes up over it (as `wand::open` does).
pub fn open_over_game(app: &AppHandle, pids: Vec<u32>) {
    crate::overlay::hide(app);
    crate::window::resume_game(app, &pids);
    crate::overlay::after_resume(app, pids);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        show(&app);
    });
}

/// Types `text` where the focus is.
pub fn type_text(text: &str) {
    assist::type_text(text);
}

/// What the watcher does after a reading.
#[derive(Debug, PartialEq)]
enum Step {
    Nothing,
    Open(TextField),
    Close,
}

/// The text-field watcher's memory between readings.
#[derive(Default)]
struct Watcher {
    /// Consecutive readings without a focused field.
    misses: u32,
    /// The field of the previous reading.
    last: Option<[i32; 4]>,
    /// The field the user closed the keyboard on: not opened again there.
    dismissed: Option<[i32; 4]>,
}

impl Watcher {
    /// `open`: the keyboard is up; `auto`: the watcher opened it;
    /// `dismissed_now`: the user closed it since the previous reading.
    fn step(&mut self, field: Option<TextField>, open: bool, auto: bool, dismissed_now: bool) -> Step {
        let rect = field.map(|f| f.rect);
        if dismissed_now {
            // The field it was open on: when Enter moved the focus on, the
            // next field still gets the keyboard.
            self.dismissed = self.last.or(rect);
        }
        self.last = rect;
        match field {
            Some(field) => {
                self.misses = 0;
                if self.dismissed.is_some_and(|d| d != field.rect) {
                    self.dismissed = None;
                }
                if !open && self.dismissed.is_none() {
                    Step::Open(field)
                } else {
                    Step::Nothing
                }
            }
            None => {
                self.misses = self.misses.saturating_add(1);
                if self.misses < CLOSE_MISSES {
                    return Step::Nothing;
                }
                // The focus really left the field: it may open there again.
                self.dismissed = None;
                if auto {
                    Step::Close
                } else {
                    Step::Nothing
                }
            }
        }
    }
}

/// Opens the keyboard by itself when a text field takes the focus in a
/// store window during a hand-off, and closes it when the focus leaves.
/// Once the user closes it, it stays closed until the focus moves to
/// another field. Best effort: apps without an accessible tree (Steam's
/// CEF) show a pane instead of a field, and then Y opens it by hand.
pub fn watch(app: AppHandle) {
    std::thread::spawn(move || {
        let mut watcher = Watcher::default();
        loop {
            std::thread::sleep(WATCH_INTERVAL);
            let wanted = {
                let state = app.state::<crate::AppState>();
                state.db.lock().settings.keyboard_auto
            };
            let typing = wanted && assist::is_active() && assist::types_keys() && !crate::window::is_foreground(&app);
            if !typing {
                if AUTO.load(Ordering::SeqCst) {
                    hide(&app);
                }
                watcher = Watcher::default();
                DISMISSED.store(false, Ordering::SeqCst);
                continue;
            }
            let field = uia::focused_text_field();
            let dismissed_now = DISMISSED.swap(false, Ordering::SeqCst);
            match watcher.step(field, is_open(), AUTO.load(Ordering::SeqCst), dismissed_now) {
                Step::Open(field) => {
                    AUTO.store(true, Ordering::SeqCst);
                    show_placed(&app, placement_of(&app, Some(field)), field.password);
                }
                Step::Close => hide(&app),
                Step::Nothing => {}
            }
        }
    });
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };

    /// Clicking or showing the window never makes it the foreground one.
    pub fn never_activate(raw: isize) {
        let hwnd = HWND(raw as *mut std::ffi::c_void);
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_moves_up_when_the_field_is_low() {
        assert_eq!(placement_for(0, 1080, None), Placement::Bottom);
        assert_eq!(placement_for(0, 1080, Some(300)), Placement::Bottom);
        assert_eq!(placement_for(0, 1080, Some(540)), Placement::Bottom);
        assert_eq!(placement_for(0, 1080, Some(541)), Placement::Top);
        // A monitor that doesn't start at y = 0.
        assert_eq!(placement_for(-1080, 1080, Some(-400)), Placement::Top);
        assert_eq!(placement_for(-1080, 1080, Some(-800)), Placement::Bottom);
    }

    fn field(top: i32) -> Option<TextField> {
        Some(TextField { rect: [100, top, 600, top + 40], password: false })
    }

    #[test]
    fn watcher_opens_on_a_field_and_closes_when_it_goes() {
        let mut w = Watcher::default();
        assert_eq!(w.step(field(200), false, false, false), Step::Open(field(200).unwrap()));
        assert_eq!(w.step(field(200), true, true, false), Step::Nothing);
        // One reading without a field is a flicker.
        assert_eq!(w.step(None, true, true, false), Step::Nothing);
        assert_eq!(w.step(field(200), true, true, false), Step::Nothing);
        assert_eq!(w.step(None, true, true, false), Step::Nothing);
        assert_eq!(w.step(None, true, true, false), Step::Close);
        // Opened by hand (Y): the watcher leaves it alone.
        assert_eq!(w.step(None, true, false, false), Step::Nothing);
        assert_eq!(w.step(None, true, false, false), Step::Nothing);
    }

    #[test]
    fn closed_by_the_user_it_stays_closed_on_that_field() {
        let mut w = Watcher::default();
        assert_eq!(w.step(field(200), false, false, false), Step::Open(field(200).unwrap()));
        // B on a field that keeps the focus.
        assert_eq!(w.step(field(200), false, false, true), Step::Nothing);
        assert_eq!(w.step(field(200), false, false, false), Step::Nothing);
        // One flicker doesn't count as leaving the field.
        assert_eq!(w.step(None, false, false, false), Step::Nothing);
        assert_eq!(w.step(field(200), false, false, false), Step::Nothing);
        // Another field opens it again.
        assert_eq!(w.step(field(300), false, false, false), Step::Open(field(300).unwrap()));
        // Closed there, then the focus leaves and comes back: open again.
        assert_eq!(w.step(field(300), false, false, true), Step::Nothing);
        assert_eq!(w.step(None, false, false, false), Step::Nothing);
        assert_eq!(w.step(None, false, false, false), Step::Nothing);
        assert_eq!(w.step(field(300), false, false, false), Step::Open(field(300).unwrap()));
    }

    #[test]
    fn enter_that_moves_the_focus_opens_on_the_next_field() {
        let mut w = Watcher::default();
        assert_eq!(w.step(field(200), false, false, false), Step::Open(field(200).unwrap()));
        // Enter closed it and the focus went to the next field.
        assert_eq!(w.step(field(300), false, false, true), Step::Open(field(300).unwrap()));
    }

    #[test]
    fn key_names_from_the_frontend() {
        assert_eq!(key_named("enter"), Some(Key::Enter));
        assert_eq!(key_named("backspace"), Some(Key::Backspace));
        assert_eq!(key_named("escape"), Some(Key::Escape));
        assert_eq!(key_named("tab"), Some(Key::Tab));
        assert_eq!(key_named("space"), Some(Key::Space));
        assert_eq!(key_named("Enter"), None);
        assert_eq!(key_named("alt+w"), None);
    }
}
