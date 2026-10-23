//! On-screen volume indicator: a small click-through window at the bottom of
//! the game's screen that pops up whenever the system volume changes (volume
//! keys, controller combo, Bluetooth headphones taking over), in place of
//! the Windows flyout that isn't there in console mode. Like the overlay it
//! can't appear over true exclusive-fullscreen games.

// Shown from the audio callback thread, which only exists on Windows.
#![cfg_attr(not(windows), allow(dead_code))]

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

use crate::system::{self, Source, Volume};

pub const LABEL: &str = "osd";

/// Logical size of the indicator; the page sets its own root font size.
const WIDTH: f64 = 420.0;
const HEIGHT: f64 = 88.0;
/// Distance from the bottom edge of the screen.
const MARGIN: f64 = 56.0;
/// How long the indicator stays after the last change.
const SHOW_FOR: std::time::Duration = std::time::Duration::from_millis(1800);
/// Volume keys step like Windows does; the controller takes bigger steps
/// like the quick menu.
pub const KEY_STEP: f32 = 0.02;
pub const PAD_STEP: f32 = 0.05;

/// Bumped on every show, so only the newest auto-hide timer hides.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OsdPayload {
    pub level: f32,
    pub muted: bool,
    /// Shown instead of "Volume": the new output after a device change.
    pub label: Option<String>,
}

/// Pushed to every window whenever the system volume changes.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct VolumeEvent {
    pub level: f32,
    pub muted: bool,
    /// Changed from GameMode's own quick menu or overlay, which already show it.
    pub own: bool,
}

/// Whether a volume change deserves the indicator. Own changes come from a
/// visible GameMode panel; with Explorer around and no game running, Windows
/// shows its own flyout.
pub fn wants(own: bool, console: bool, explorer: bool, playing: bool, enabled: bool) -> bool {
    enabled && !own && (console || !explorer || playing)
}

/// The next level after a step: snapped to the step grid so repeated presses
/// land on round numbers, clamped to 0 – 1, in whole percent.
pub fn step(level: f32, delta: f32) -> f32 {
    let unit = delta.abs();
    let next = if unit == 0.0 { level } else { ((level + delta) / unit).round() * unit };
    ((next * 100.0).round() / 100.0).clamp(0.0, 1.0)
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// Creates the hidden indicator window. Windows only.
pub fn create(app: &AppHandle) {
    #[cfg(windows)]
    {
        use tauri::{WebviewUrl, WebviewWindowBuilder};
        let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html?osd=1".into()))
            .title("GameMode")
            .inner_size(WIDTH, HEIGHT)
            .transparent(true)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .shadow(false)
            .focusable(false)
            .focused(false)
            .visible(false)
            .build();
        match built {
            Ok(w) => {
                // Clicks go through to the game underneath.
                let _ = w.set_ignore_cursor_events(true);
            }
            Err(err) => eprintln!("osd window: {err}"),
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

fn enabled(app: &AppHandle) -> bool {
    app.state::<crate::AppState>().db.lock().settings.osd_volume
}

fn playing(app: &AppHandle) -> bool {
    app.state::<crate::AppState>()
        .launcher
        .current()
        .is_some_and(|s| s.phase == crate::launcher::Phase::Running)
}

/// The controller combo (View + D-pad up/down) works during a game only.
pub fn combo_active(app: &AppHandle) -> bool {
    enabled(app) && playing(app)
}

/// Volume changed, from anywhere: every window learns the new value and the
/// indicator comes up when nothing else shows it.
pub fn on_volume(app: &AppHandle, volume: Volume, source: Option<Source>) {
    let own = source == Some(Source::Ui);
    let _ = app.emit("audio://volume", VolumeEvent { level: volume.level, muted: volume.muted, own });
    let console = crate::shell::status().active;
    if wants(own, console, crate::shell::explorer_running(), playing(app), enabled(app)) {
        show(app, OsdPayload { level: volume.level, muted: volume.muted, label: None });
    }
}

/// The default output changed (headphones connected, HDMI audio back): say
/// which one is playing now, with its volume.
pub fn on_device(app: &AppHandle, name: String, volume: Volume) {
    let _ = app.emit("audio://device", serde_json::json!({ "name": name }));
    if enabled(app) {
        show(app, OsdPayload { level: volume.level, muted: volume.muted, label: Some(name) });
    }
}

/// Steps the volume from a shortcut (keys or controller); unmutes first,
/// like the Windows keys do. The indicator follows from the audio callback.
pub fn nudge(app: &AppHandle, delta: f32) {
    let _ = app;
    let Ok(current) = system::volume() else { return };
    if current.muted {
        let _ = system::set_muted_from(false, Source::Shortcut);
    }
    let _ = system::set_volume_from(step(current.level, delta), Source::Shortcut);
}

pub fn toggle_mute(app: &AppHandle) {
    let _ = app;
    if let Ok(current) = system::volume() {
        let _ = system::set_muted_from(!current.muted, Source::Shortcut);
    }
}

/// Puts the indicator at the bottom centre of the screen the foreground
/// window (the game) is on.
fn place(w: &WebviewWindow) {
    #[cfg(windows)]
    {
        use tauri::{PhysicalPosition, PhysicalSize};
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        // With no foreground window this is the primary monitor.
        let found = unsafe {
            GetMonitorInfoW(MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONEAREST), &mut info).as_bool()
        };
        if !found {
            return;
        }
        let screen = info.rcMonitor;
        let cx = f64::from(screen.left + screen.right) / 2.0;
        let cy = f64::from(screen.top + screen.bottom) / 2.0;
        let scale = w.monitor_from_point(cx, cy).ok().flatten().map(|m| m.scale_factor()).unwrap_or(1.0);
        let (pw, ph) = ((WIDTH * scale) as u32, (HEIGHT * scale) as u32);
        let x = cx as i32 - (pw / 2) as i32;
        let y = screen.bottom - ph as i32 - (MARGIN * scale) as i32;
        let _ = w.set_size(PhysicalSize::new(pw, ph));
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
    #[cfg(not(windows))]
    let _ = w;
}

/// Shows (or refreshes) the indicator without taking focus from the game;
/// it goes away by itself after `SHOW_FOR`.
pub fn show(app: &AppHandle, payload: OsdPayload) {
    let Some(w) = window(app) else { return };
    place(&w);
    let _ = app.emit_to(LABEL, "osd://show", payload);
    let _ = w.show();
    #[cfg(windows)]
    if let Ok(hwnd) = w.hwnd() {
        crate::window::raise_topmost_raw(hwnd.0 as isize);
    }
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SHOW_FOR);
        if GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation {
            hide(&app);
        }
    });
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = window(app) {
        let _ = w.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_snap_to_the_grid_and_clamp() {
        assert_eq!(step(0.6, 0.05), 0.65);
        assert_eq!(step(0.333, 0.05), 0.4);
        assert_eq!(step(0.98, 0.05), 1.0);
        assert_eq!(step(0.03, -0.05), 0.0);
        assert_eq!(step(0.5, 0.02), 0.52);
        assert_eq!(step(0.5, -0.02), 0.48);
        assert_eq!(step(0.5, 0.0), 0.5);
    }

    #[test]
    fn own_changes_and_windows_flyout_skip_the_indicator() {
        // Keys in console mode, no game: ours to show.
        assert!(wants(false, true, false, false, true));
        // Explorer in the background in console mode still shows nothing.
        assert!(wants(false, true, true, false, true));
        // Desktop mode, Windows' flyout handles it.
        assert!(!wants(false, false, true, false, true));
        // Desktop mode but a game is up: flyouts don't reach it.
        assert!(wants(false, false, true, true, true));
        // The quick menu or the overlay already shows the bar.
        assert!(!wants(true, true, false, true, true));
        // Switched off.
        assert!(!wants(false, true, false, true, false));
    }
}
