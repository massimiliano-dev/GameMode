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
/// shows its own flyout — except for keys GameMode itself swallowed
/// (`shortcut`), which never reach Windows.
pub fn wants(own: bool, shortcut: bool, console: bool, explorer: bool, playing: bool, enabled: bool) -> bool {
    enabled && !own && (shortcut || console || !explorer || playing)
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
    let shortcut = source == Some(Source::Shortcut);
    let _ = app.emit("audio://volume", VolumeEvent { level: volume.level, muted: volume.muted, own });
    let console = crate::shell::status().active;
    if wants(own, shortcut, console, crate::shell::explorer_running(), playing(app), enabled(app)) {
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
/// window (the game) is on and shows it without activating it.
///
/// Done with raw `SetWindowPos` rather than `show()`: tao shows windows with
/// `SW_SHOW`, which activates them and could steal focus from the game.
/// Moving to a monitor with a different DPI makes tao rescale (and hide) the
/// window on `WM_DPICHANGED`, so the move is repeated until it sticks.
#[cfg(windows)]
fn place_and_show(w: &WebviewWindow) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, IsWindowVisible, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    };
    let Ok(raw) = w.hwnd() else { return };
    let hwnd = HWND(raw.0 as *mut core::ffi::c_void);
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
    let (pw, ph) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32);
    let x = cx as i32 - pw / 2;
    let y = screen.bottom - ph - (MARGIN * scale) as i32;
    let wanted = RECT { left: x, top: y, right: x + pw, bottom: y + ph };
    for _ in 0..3 {
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, pw, ph, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        let mut rect = RECT::default();
        let placed = unsafe { GetWindowRect(hwnd, &mut rect).is_ok() && IsWindowVisible(hwnd).as_bool() };
        if placed && rect == wanted {
            break;
        }
    }
}

/// Shows (or refreshes) the indicator without taking focus from the game;
/// it goes away by itself after `SHOW_FOR`.
pub fn show(app: &AppHandle, payload: OsdPayload) {
    let Some(w) = window(app) else { return };
    // Bumped first, so an older timer about to fire no longer matches.
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let _ = app.emit_to(LABEL, "osd://show", payload);
    #[cfg(windows)]
    place_and_show(&w);
    #[cfg(not(windows))]
    let _ = w.show();
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SHOW_FOR);
        // Checked on the main thread, in line with the window calls of a
        // newer show, so a stale timer can't hide it.
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation {
                hide(&handle);
            }
        });
    });
}

/// Hidden the same raw way it was shown: tao never saw the window become
/// visible (`place_and_show` bypasses it), so its own `hide()` thinks
/// there's nothing to do and the indicator would stay up for good.
pub fn hide(app: &AppHandle) {
    let Some(w) = window(app) else { return };
    #[cfg(windows)]
    if let Ok(raw) = w.hwnd() {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        unsafe {
            let _ = ShowWindow(windows::Win32::Foundation::HWND(raw.0 as *mut core::ffi::c_void), SW_HIDE);
        }
    }
    let _ = w.hide();
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
        assert!(wants(false, false, true, false, false, true));
        // Explorer in the background in console mode: still ours.
        assert!(wants(false, false, true, true, false, true));
        // Desktop mode, a change from another app: Windows' flyout handles it.
        assert!(!wants(false, false, false, true, false, true));
        // Desktop mode but keys GameMode swallowed: Windows shows nothing.
        assert!(wants(false, true, false, true, false, true));
        // Desktop mode but a game is up: flyouts don't reach it.
        assert!(wants(false, false, false, true, true, true));
        // The quick menu or the overlay already shows the bar.
        assert!(!wants(true, false, true, false, true, true));
        // Switched off.
        assert!(!wants(false, true, true, false, true, false));
    }
}
