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
/// How long the overlay takes focus back from any window after opening.
const FOCUS_FIGHT: std::time::Duration = std::time::Duration::from_secs(3);
/// Apps that react to Guide themselves: the overlay takes focus back from
/// them for as long as it is open.
const GUIDE_GRABBERS: &[&str] = &["steam.exe", "steamwebhelper.exe", "gamebar.exe", "gamebarftserver.exe"];

/// Steam's windows: these are minimized out of the way rather than fought
/// over, since Steam keeps reclaiming the foreground after Guide.
const STEAM: &[&str] = &["steam.exe", "steamwebhelper.exe"];

/// Gets Steam out of the way if Guide brought it in front of the game.
fn dismiss_steam() -> bool {
    #[cfg(windows)]
    {
        crate::window::minimize_foreground_of(STEAM)
    }
    #[cfg(not(windows))]
    {
        let _ = STEAM;
        false
    }
}

/// The foreground window belongs to an app that grabs Guide.
fn guide_grabber_in_front() -> bool {
    #[cfg(windows)]
    {
        crate::window::foreground_process_name().is_some_and(|n| GUIDE_GRABBERS.contains(&n.as_str()))
    }
    #[cfg(not(windows))]
    {
        let _ = GUIDE_GRABBERS;
        false
    }
}

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
            log::warn!("overlay window: {err}");
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

/// After the overlay closes back to the game: Steam answers Guide too, so
/// if it jumped in front, it goes back down and the game gets the screen.
pub fn after_resume(app: &AppHandle, pids: Vec<u32>) {
    let app = app.clone();
    std::thread::spawn(move || {
        for _ in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if dismiss_steam() {
                crate::window::resume_game(&app, &pids);
            }
        }
    });
}

/// Opens the overlay after Guide: Steam and Game Bar react to the same
/// press, so wait for them to make their move, then clear Steam away and
/// come up on top.
pub fn show_after_guide(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        dismiss_steam();
        show(&app);
    });
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
            if started.elapsed() < FOCUS_FIGHT || guide_grabber_in_front() {
                dismiss_steam();
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

/// The screen changed (resolution, a TV coming back): the open overlay
/// covers the new screen.
pub fn fit_screen(app: &AppHandle) {
    let Some(w) = window(app) else { return };
    if let Ok(Some(monitor)) = w.primary_monitor() {
        let _ = w.set_position(*monitor.position());
        let _ = w.set_size(*monitor.size());
    }
}

pub fn hide(app: &AppHandle) {
    VISIBLE.store(false, std::sync::atomic::Ordering::SeqCst);
    if let Some(w) = window(app) {
        let _ = w.hide();
    }
    // The mixer stops polling while the overlay is away.
    let _ = app.emit_to(LABEL, "overlay://close", ());
}
