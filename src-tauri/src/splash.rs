//! Startup in console mode: until WebView2 has painted the first screen the
//! display would stay black for a second or more. A native Win32 window with
//! the logo covers that gap from the very first instant; the web UI calls
//! `app_ready` once its first screen is up, the main window (created hidden)
//! is shown underneath and the splash fades away.
//!
//! The same "ready" signal holds back the heavy startup work (library scan,
//! Steam, Store package catalog) so it doesn't compete with the first screen,
//! and ends the raised process priority of a shell start.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::AppHandle;

/// The main window shows up by this time after start even if the web UI
/// never says it's ready.
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// Pause after the first screen before the heavy startup work begins.
const SETTLE: Duration = Duration::from_secs(2);
/// Last resort for the splash itself, should nothing ever close it.
#[cfg_attr(not(windows), allow(dead_code))]
const SPLASH_MAX: Duration = Duration::from_secs(20);
/// The splash stays opaque this long once the main window is shown (its
/// first frame reaches the screen), then fades out.
#[cfg_attr(not(windows), allow(dead_code))]
const HOLD_MS: u64 = 150;
#[cfg_attr(not(windows), allow(dead_code))]
const FADE_MS: u64 = 300;
/// Logo fade-in at the start.
#[cfg_attr(not(windows), allow(dead_code))]
const LOGO_IN_MS: u64 = 350;
/// One cycle of the pulsing dots under the logo.
#[cfg_attr(not(windows), allow(dead_code))]
const PULSE_MS: u64 = 1200;

/// `tauri.conf.json` background and the UI accent.
#[cfg_attr(not(windows), allow(dead_code))]
const BACKGROUND: [u8; 3] = [0x07, 0x09, 0x0d];
#[cfg_attr(not(windows), allow(dead_code))]
const ACCENT: [u8; 3] = [0x5e, 0xf2, 0xc8];

const IDLE: u8 = 0;
const SHOWING: u8 = 1;
const CLOSING: u8 = 2;
const CANCELLED: u8 = 3;

static STARTED: OnceLock<Instant> = OnceLock::new();
static PHASE: AtomicU8 = AtomicU8::new(IDLE);
static READY: Mutex<bool> = Mutex::new(false);
static READY_CHANGED: Condvar = Condvar::new();
/// The main window stays hidden at "ready" (desktop once: GameMode waits in the tray).
static STAY_HIDDEN: AtomicBool = AtomicBool::new(false);
static PRIORITY_RAISED: AtomicBool = AtomicBool::new(false);

/// How this start looks, as far as the splash is concerned.
#[derive(Clone, Copy, Debug, Default)]
struct Start {
    safe_mode: bool,
    /// Another GameMode is running: this copy hands over and exits.
    other_instance: bool,
    /// GameMode is (very likely) the shell of this session.
    console: bool,
    autostart: bool,
    /// Fullscreen applies (release build with `start_fullscreen` on).
    fullscreen: bool,
}

/// Only starts that cover the whole screen get a splash: in a window on the
/// desktop the black gap doesn't exist.
fn wanted(s: Start) -> bool {
    !s.safe_mode && !s.other_instance && (s.console || s.autostart || s.fullscreen)
}

/// `settings.startFullscreen` from `library.json`; on (the default) when
/// missing or unreadable.
fn start_fullscreen(json: &[u8]) -> bool {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Data {
        settings: Settings,
    }
    #[derive(serde::Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Settings {
        start_fullscreen: bool,
    }
    impl Default for Settings {
        fn default() -> Self {
            Settings { start_fullscreen: true }
        }
    }
    serde_json::from_slice::<Data>(json).map(|d| d.settings.start_fullscreen).unwrap_or(true)
}

/// Called first thing at startup, before Tauri: raises the priority of a
/// shell start and puts the splash up when wanted.
pub fn begin(identifier: &str) {
    let _ = STARTED.set(Instant::now());
    let other_instance = imp::other_instance(identifier);
    let console = !other_instance && (crate::shell::likely_shell_start() || crate::shell::recovered_count() > 0);
    if console && imp::raise_priority() {
        PRIORITY_RAISED.store(true, Ordering::SeqCst);
    }
    let mut start = Start {
        safe_mode: crate::shell::safe_mode(),
        other_instance,
        console,
        autostart: crate::autostart::launched_at_login(),
        fullscreen: false,
    };
    // The settings file is read only when nothing else decided already.
    if !wanted(start) && !cfg!(debug_assertions) {
        let file = dirs::data_dir().map(|d| d.join(identifier).join("library.json"));
        start.fullscreen = file.and_then(|f| std::fs::read(f).ok()).is_none_or(|bytes| start_fullscreen(&bytes));
    }
    if wanted(start) && imp::supported() {
        PHASE.store(SHOWING, Ordering::SeqCst);
        imp::spawn();
    }
}

/// The main window must stay hidden when ready (GameMode starts in the
/// tray): no splash either.
pub fn stay_hidden() {
    STAY_HIDDEN.store(true, Ordering::SeqCst);
    let _ = PHASE.compare_exchange(SHOWING, CANCELLED, Ordering::SeqCst, Ordering::SeqCst);
}

/// The splash covers the screen right now.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn showing() -> bool {
    matches!(PHASE.load(Ordering::SeqCst), SHOWING | CLOSING)
}

/// The web UI painted its first screen (or gave up waiting): shows the main
/// window and lets the splash fade out. Only the first call counts.
pub fn ready(app: &AppHandle) {
    {
        let mut ready = READY.lock().unwrap_or_else(|e| e.into_inner());
        if *ready {
            return;
        }
        *ready = true;
    }
    READY_CHANGED.notify_all();
    log::info!("first screen ready after {} ms", started().elapsed().as_millis());
    if !STAY_HIDDEN.load(Ordering::SeqCst) {
        show_main(app);
    }
    let _ = PHASE.compare_exchange(SHOWING, CLOSING, Ordering::SeqCst, Ordering::SeqCst);
}

fn show_main(app: &AppHandle) {
    let Some(window) = crate::window::main_window(app) else { return };
    let _ = window.show();
    let _ = window.set_focus();
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        crate::window::force_foreground_raw(hwnd.0 as isize);
    }
}

/// From setup: the main window shows up after `READY_TIMEOUT` whatever the
/// web UI does, and the process priority goes back to normal afterwards.
pub fn watch(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if !wait_ready() {
            log::warn!("no first screen after {} s: showing the window anyway", READY_TIMEOUT.as_secs());
            ready(&app);
        }
        std::thread::sleep(SETTLE);
        if PRIORITY_RAISED.swap(false, Ordering::SeqCst) {
            imp::normal_priority();
        }
    });
}

/// Blocks until the first screen is up and has had a moment for itself (or
/// the timeout passed).
pub fn wait_settled() {
    wait_ready();
    std::thread::sleep(SETTLE);
}

/// Runs `f` on its own thread once the first screen is up.
pub fn after_startup(f: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        wait_settled();
        f();
    });
}

fn started() -> Instant {
    *STARTED.get_or_init(Instant::now)
}

/// Waits for `ready`, at most until `READY_TIMEOUT` after start. True when ready.
fn wait_ready() -> bool {
    let deadline = started() + READY_TIMEOUT;
    let mut ready = READY.lock().unwrap_or_else(|e| e.into_inner());
    while !*ready {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        ready = READY_CHANGED.wait_timeout(ready, left).unwrap_or_else(|e| e.into_inner()).0;
    }
    true
}

// ---------------------------------------------------------------- drawing

/// Opacity of the closing splash `ms` after it was told to go; None once gone.
#[cfg_attr(not(windows), allow(dead_code))]
fn fade_alpha(ms: u64) -> Option<u8> {
    if ms < HOLD_MS {
        return Some(255);
    }
    let t = (ms - HOLD_MS) as f32 / FADE_MS as f32;
    (t < 1.0).then(|| (255.0 * (1.0 - ease(t))).round() as u8)
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Logo opacity during its fade-in.
#[cfg_attr(not(windows), allow(dead_code))]
fn logo_alpha(ms: u64) -> u8 {
    (255.0 * ease(ms as f32 / LOGO_IN_MS as f32)).round() as u8
}

/// Opacity of dot `index` at `ms`: a slow wave running left to right.
#[cfg_attr(not(windows), allow(dead_code))]
fn pulse(ms: u64, index: u32) -> u8 {
    let phase = ((ms + PULSE_MS - u64::from(index) * PULSE_MS / 6) % PULSE_MS) as f32 / PULSE_MS as f32;
    let wave = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
    (255.0 * (0.22 + 0.78 * wave)).round() as u8
}

/// Logo side for a screen `height` pixels tall.
#[cfg_attr(not(windows), allow(dead_code))]
fn logo_side(height: i32) -> i32 {
    (height / 6).clamp(96, 420)
}

/// Bilinear resize of straight RGBA into premultiplied BGRA (what GDI's
/// `AlphaBlend` wants).
#[cfg_attr(not(windows), allow(dead_code))]
fn resize_premultiplied(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        return out;
    }
    let premul = |x: usize, y: usize| -> [f32; 4] {
        let i = (y * sw + x) * 4;
        let a = f32::from(src[i + 3]) / 255.0;
        [f32::from(src[i + 2]) * a, f32::from(src[i + 1]) * a, f32::from(src[i]) * a, f32::from(src[i + 3])]
    };
    for y in 0..dh {
        let fy = ((y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for x in 0..dw {
            let fx = ((x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let (x0, tx) = (fx.floor() as usize, fx.fract());
            let x1 = (x0 + 1).min(sw - 1);
            let (a, b, c, d) = (premul(x0, y0), premul(x1, y0), premul(x0, y1), premul(x1, y1));
            let o = (y * dw + x) * 4;
            for k in 0..4 {
                let top = a[k] + (b[k] - a[k]) * tx;
                let bottom = c[k] + (d[k] - c[k]) * tx;
                out[o + k] = (top + (bottom - top) * ty).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// An antialiased disc of `rgb`, `size` pixels wide, as premultiplied BGRA.
#[cfg_attr(not(windows), allow(dead_code))]
fn disc(size: usize, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    let r = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 + 0.5 - r, y as f32 + 0.5 - r);
            let coverage = (r - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let o = (y * size + x) * 4;
            out[o] = (f32::from(rgb[2]) * coverage).round() as u8;
            out[o + 1] = (f32::from(rgb[1]) * coverage).round() as u8;
            out[o + 2] = (f32::from(rgb[0]) * coverage).round() as u8;
            out[o + 3] = (255.0 * coverage).round() as u8;
        }
    }
    out
}

/// The logo (straight RGBA) and its size.
#[cfg_attr(not(windows), allow(dead_code))]
fn decode_logo() -> Option<(Vec<u8>, usize, usize)> {
    const LOGO: &[u8] = include_bytes!("../icons/128x128@2x.png");
    let mut decoder = png::Decoder::new(std::io::Cursor::new(LOGO));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
        png::ColorType::Rgb => buf[..w * h * 3].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        _ => return None,
    };
    Some((rgba, w, h))
}

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    use windows::core::{w, HSTRING};
    use windows::Win32::Foundation::{CloseHandle, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        AlphaBlend, BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush,
        DeleteDC, DeleteObject, EndPaint, ExcludeClipRect, FillRect, GetDC, GetMonitorInfoW, InvalidateRect,
        MonitorFromPoint, ReleaseDC, SelectClipRgn, SelectObject, UpdateWindow, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HBRUSH, HDC, HGDIOBJ, MONITORINFO,
        MONITOR_DEFAULTTOPRIMARY, PAINTSTRUCT, SRCCOPY,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenMutexW, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        SYNCHRONIZATION_SYNCHRONIZE,
    };
    use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, LoadCursorW,
        PostQuitMessage, RegisterClassExW, SetForegroundWindow, SetLayeredWindowAttributes, SetTimer, SetWindowPos,
        ShowWindow, TranslateMessage, UnregisterClassW, HWND_TOPMOST, IDC_ARROW, LWA_ALPHA, MSG, SWP_NOACTIVATE, SW_SHOW,
        WM_DESTROY, WM_DISPLAYCHANGE, WM_ERASEBKGND, WM_PAINT, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST, WS_POPUP,
    };

    use super::*;

    const CLASS: windows::core::PCWSTR = w!("GameModeSplash");
    const TIMER: usize = 1;
    const FRAME_MS: u32 = 15;
    const DOTS: u32 = 3;

    pub fn supported() -> bool {
        true
    }

    /// The single-instance plugin's mutex exists: this copy is about to
    /// hand its arguments over and exit.
    pub fn other_instance(identifier: &str) -> bool {
        let name = HSTRING::from(format!("{identifier}-sim"));
        match unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, &name) } {
            Ok(handle) => {
                let _ = unsafe { CloseHandle(handle) };
                true
            }
            Err(_) => false,
        }
    }

    pub fn raise_priority() -> bool {
        unsafe { SetPriorityClass(GetCurrentProcess(), ABOVE_NORMAL_PRIORITY_CLASS).is_ok() }
    }

    pub fn normal_priority() {
        let _ = unsafe { SetPriorityClass(GetCurrentProcess(), NORMAL_PRIORITY_CLASS) };
    }

    pub fn spawn() {
        std::thread::Builder::new()
            .name("splash".into())
            .spawn(|| {
                if let Err(err) = run() {
                    log::warn!("splash: {err}");
                }
                PHASE.store(CANCELLED, Ordering::SeqCst);
            })
            .ok();
    }

    fn colorref([r, g, b]: [u8; 3]) -> COLORREF {
        COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
    }

    /// A premultiplied BGRA image in its own memory DC, ready for `AlphaBlend`.
    struct Sprite {
        dc: HDC,
        bitmap: HBITMAP,
        old: HGDIOBJ,
        size: (i32, i32),
    }

    impl Sprite {
        fn new(bgra: &[u8], w: i32, h: i32) -> Option<Sprite> {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            unsafe {
                let mut bits = std::ptr::null_mut();
                let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
                if bits.is_null() {
                    let _ = DeleteObject(bitmap.into());
                    return None;
                }
                let len = (w * h * 4) as usize;
                std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, len.min(bgra.len()));
                let dc = CreateCompatibleDC(None);
                let old = SelectObject(dc, bitmap.into());
                Some(Sprite { dc, bitmap, old, size: (w, h) })
            }
        }

        fn draw(&self, target: HDC, x: i32, y: i32, alpha: u8) {
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: alpha,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let (w, h) = self.size;
            unsafe {
                let _ = AlphaBlend(target, x, y, w, h, self.dc, 0, 0, w, h, blend);
            }
        }
    }

    impl Drop for Sprite {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.dc, self.old);
                let _ = DeleteDC(self.dc);
                let _ = DeleteObject(self.bitmap.into());
            }
        }
    }

    /// Everything the window paints. Only `area` (logo and dots) changes;
    /// the rest is plain background.
    struct Scene {
        started: Instant,
        closing_since: Option<Instant>,
        background: HBRUSH,
        area: RECT,
        logo: Option<Sprite>,
        dot: Sprite,
        dot_step: i32,
        /// Back buffer for `area`.
        buffer: HDC,
        buffer_bitmap: HBITMAP,
        buffer_old: HGDIOBJ,
    }

    impl Scene {
        fn new(screen: RECT, background: HBRUSH) -> Option<Scene> {
            let (sw, sh) = (screen.right - screen.left, screen.bottom - screen.top);
            let side = logo_side(sh);
            let logo = decode_logo().and_then(|(rgba, w, h)| {
                let scaled = resize_premultiplied(&rgba, w, h, side as usize, side as usize);
                Sprite::new(&scaled, side, side)
            });
            let dot_size = (side / 18).max(6);
            let dot = Sprite::new(&disc(dot_size as usize, ACCENT), dot_size, dot_size)?;
            let gap = side / 3;
            let left = (sw - side) / 2;
            let top = (sh - side) / 2;
            // Area relative to the window (which sits on the monitor's origin).
            let area = RECT { left, top, right: left + side, bottom: top + side + gap + dot_size };
            let (aw, ah) = (area.right - area.left, area.bottom - area.top);
            unsafe {
                let screen_dc = GetDC(None);
                let buffer = CreateCompatibleDC(Some(screen_dc));
                let buffer_bitmap = CreateCompatibleBitmap(screen_dc, aw, ah);
                ReleaseDC(None, screen_dc);
                let buffer_old = SelectObject(buffer, buffer_bitmap.into());
                Some(Scene {
                    started: Instant::now(),
                    closing_since: None,
                    background,
                    area,
                    logo,
                    dot,
                    dot_step: dot_size * 5 / 2,
                    buffer,
                    buffer_bitmap,
                    buffer_old,
                })
            }
        }

        /// Paints `area` into the back buffer, then onto `target`.
        fn paint_area(&self, target: HDC) {
            let ms = self.started.elapsed().as_millis() as u64;
            let (aw, ah) = (self.area.right - self.area.left, self.area.bottom - self.area.top);
            let logo_a = logo_alpha(ms);
            unsafe {
                FillRect(self.buffer, &RECT { left: 0, top: 0, right: aw, bottom: ah }, self.background);
                if let Some(logo) = &self.logo {
                    logo.draw(self.buffer, 0, 0, logo_a);
                }
                let (dw, dh) = self.dot.size;
                let row = self.dot_step * (DOTS as i32 - 1) + dw;
                let x0 = (aw - row) / 2;
                for i in 0..DOTS {
                    let a = (u32::from(pulse(ms, i)) * u32::from(logo_a) / 255) as u8;
                    self.dot.draw(self.buffer, x0 + i as i32 * self.dot_step, ah - dh, a);
                }
                let _ = BitBlt(target, self.area.left, self.area.top, aw, ah, Some(self.buffer), 0, 0, SRCCOPY);
            }
        }
    }

    impl Drop for Scene {
        fn drop(&mut self) {
            unsafe {
                SelectObject(self.buffer, self.buffer_old);
                let _ = DeleteDC(self.buffer);
                let _ = DeleteObject(self.buffer_bitmap.into());
            }
        }
    }

    thread_local! {
        static SCENE: RefCell<Option<Scene>> = const { RefCell::new(None) };
    }

    fn primary_screen() -> Option<RECT> {
        let mut monitor = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let primary = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            GetMonitorInfoW(primary, &mut monitor).as_bool().then_some(monitor.rcMonitor)
        }
    }

    /// The TV mode changed the resolution under the splash: cover the new screen.
    fn relayout(hwnd: HWND) {
        let Some(screen) = primary_screen() else { return };
        SCENE.with(|s| {
            let Ok(mut slot) = s.try_borrow_mut() else { return };
            let Some(old) = slot.as_ref() else { return };
            if let Some(mut scene) = Scene::new(screen, old.background) {
                scene.started = old.started;
                scene.closing_since = old.closing_since;
                *slot = Some(scene);
            }
        });
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                screen.left,
                screen.top,
                screen.right - screen.left,
                screen.bottom - screen.top,
                SWP_NOACTIVATE,
            );
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn run() -> windows::core::Result<()> {
        unsafe {
            // Real pixels on high-DPI screens, for this thread only (Tauri
            // sets the process up its own way later).
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let screen = primary_screen().ok_or_else(windows::core::Error::from_thread)?;
            let background = CreateSolidBrush(colorref(BACKGROUND));
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: background,
                lpszClassName: CLASS,
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 {
                let _ = DeleteObject(background.into());
                return Err(windows::core::Error::from_thread());
            }
            let result = (|| {
                let scene = Scene::new(screen, background).ok_or_else(windows::core::Error::from_thread)?;
                SCENE.with(|s| *s.borrow_mut() = Some(scene));
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
                    CLASS,
                    w!("GameMode"),
                    WS_POPUP,
                    screen.left,
                    screen.top,
                    screen.right - screen.left,
                    screen.bottom - screen.top,
                    None,
                    None,
                    Some(instance),
                    None,
                )?;
                // A layered window stays invisible until its opacity is set.
                SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
                let _ = UpdateWindow(hwnd);
                SetTimer(Some(hwnd), TIMER, FRAME_MS, None);
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                Ok(())
            })();
            SCENE.with(|s| s.borrow_mut().take());
            let _ = UnregisterClassW(CLASS, Some(instance));
            let _ = DeleteObject(background.into());
            result
        }
    }

    enum Tick {
        Repaint(RECT),
        Fade(u8, RECT),
        Close,
    }

    fn tick() -> Option<Tick> {
        SCENE.with(|s| {
            let mut scene = s.try_borrow_mut().ok()?;
            let scene = scene.as_mut()?;
            if PHASE.load(Ordering::SeqCst) == SHOWING && scene.started.elapsed() > SPLASH_MAX {
                let _ = PHASE.compare_exchange(SHOWING, CLOSING, Ordering::SeqCst, Ordering::SeqCst);
            }
            Some(match PHASE.load(Ordering::SeqCst) {
                CANCELLED => Tick::Close,
                CLOSING => {
                    let since = *scene.closing_since.get_or_insert_with(Instant::now);
                    match fade_alpha(since.elapsed().as_millis() as u64) {
                        Some(alpha) => Tick::Fade(alpha, scene.area),
                        None => Tick::Close,
                    }
                }
                _ => Tick::Repaint(scene.area),
            })
        })
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe {
            match msg {
                WM_TIMER if wparam.0 == TIMER => {
                    // Decided with the scene borrowed, acted on without it:
                    // DestroyWindow comes straight back in here.
                    match tick() {
                        Some(Tick::Repaint(area)) => {
                            let _ = InvalidateRect(Some(hwnd), Some(&area), false);
                        }
                        Some(Tick::Fade(alpha, area)) => {
                            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
                            let _ = InvalidateRect(Some(hwnd), Some(&area), false);
                        }
                        Some(Tick::Close) | None => {
                            let _ = DestroyWindow(hwnd);
                        }
                    }
                    LRESULT(0)
                }
                WM_ERASEBKGND => LRESULT(1),
                WM_DISPLAYCHANGE => {
                    relayout(hwnd);
                    LRESULT(0)
                }
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    let hdc = BeginPaint(hwnd, &mut ps);
                    SCENE.with(|s| {
                        if let Ok(scene) = s.try_borrow() {
                            if let Some(scene) = scene.as_ref() {
                                let a = scene.area;
                                ExcludeClipRect(hdc, a.left, a.top, a.right, a.bottom);
                                FillRect(hdc, &ps.rcPaint, scene.background);
                                SelectClipRgn(hdc, None);
                                scene.paint_area(hdc);
                            }
                        }
                    });
                    let _ = EndPaint(hwnd, &ps);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    let _ = KillTimer(Some(hwnd), TIMER);
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn supported() -> bool {
        false
    }

    pub fn other_instance(_identifier: &str) -> bool {
        false
    }

    pub fn raise_priority() -> bool {
        false
    }

    pub fn normal_priority() {}

    pub fn spawn() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splash_only_for_full_screen_starts() {
        let base = Start::default();
        assert!(!wanted(base));
        assert!(wanted(Start { console: true, ..base }));
        assert!(wanted(Start { autostart: true, ..base }));
        assert!(wanted(Start { fullscreen: true, ..base }));
        assert!(!wanted(Start { console: true, safe_mode: true, ..base }));
        assert!(!wanted(Start { console: true, other_instance: true, ..base }));
    }

    #[test]
    fn reads_start_fullscreen_with_default_on() {
        assert!(start_fullscreen(b"{}"));
        assert!(start_fullscreen(b"{\"settings\":{}}"));
        assert!(start_fullscreen(b"not json"));
        assert!(!start_fullscreen(b"{\"games\":[],\"settings\":{\"startFullscreen\":false,\"preloadSteam\":true}}"));
        assert!(start_fullscreen(b"{\"settings\":{\"startFullscreen\":true}}"));
    }

    #[test]
    fn fade_holds_then_goes() {
        assert_eq!(fade_alpha(0), Some(255));
        assert_eq!(fade_alpha(HOLD_MS - 1), Some(255));
        assert_eq!(fade_alpha(HOLD_MS), Some(255));
        let mid = fade_alpha(HOLD_MS + FADE_MS / 2).unwrap();
        assert!(mid > 100 && mid < 155, "{mid}");
        assert_eq!(fade_alpha(HOLD_MS + FADE_MS), None);
    }

    #[test]
    fn logo_fades_in_and_dots_pulse() {
        assert_eq!(logo_alpha(0), 0);
        assert_eq!(logo_alpha(LOGO_IN_MS), 255);
        assert_eq!(logo_alpha(LOGO_IN_MS * 10), 255);
        let samples: Vec<u8> = (0..PULSE_MS).step_by(20).map(|t| pulse(t, 0)).collect();
        assert!(samples.iter().min().unwrap() < &70);
        assert!(samples.iter().max().unwrap() > &240);
        // The dots are out of step.
        assert_ne!(pulse(0, 0), pulse(0, 1));
        assert_eq!(pulse(0, 1), pulse(PULSE_MS, 1));
    }

    #[test]
    fn logo_size_follows_the_screen() {
        assert_eq!(logo_side(1080), 180);
        assert_eq!(logo_side(2160), 360);
        assert_eq!(logo_side(300), 96);
        assert_eq!(logo_side(10_000), 420);
    }

    #[test]
    fn resize_premultiplies_into_bgra() {
        // 1x1 half-transparent red, scaled up.
        let out = resize_premultiplied(&[255, 0, 0, 128], 1, 1, 2, 2);
        assert_eq!(out.len(), 16);
        for px in out.chunks_exact(4) {
            assert_eq!(px, [0, 0, 128, 128]);
        }
        // Halving a 2x2 checker of opaque white and transparent averages it.
        let src = [255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255];
        let out = resize_premultiplied(&src, 2, 2, 1, 1);
        assert_eq!(out, [128, 128, 128, 128]);
        // Bad input gives a blank image, not a panic.
        assert_eq!(resize_premultiplied(&[1, 2], 4, 4, 2, 2), vec![0; 16]);
    }

    #[test]
    fn disc_is_round_and_premultiplied() {
        let d = disc(10, ACCENT);
        let px = |x: usize, y: usize| &d[(y * 10 + x) * 4..(y * 10 + x) * 4 + 4];
        assert_eq!(px(5, 5), [ACCENT[2], ACCENT[1], ACCENT[0], 255]);
        assert_eq!(px(0, 0)[3], 0);
        let edge = px(0, 5);
        assert!(edge[3] > 0 && edge[3] < 255);
        assert!(edge[0] <= edge[3] && edge[1] <= edge[3] && edge[2] <= edge[3]);
    }

    #[test]
    fn logo_decodes() {
        let (rgba, w, h) = decode_logo().unwrap();
        assert_eq!((w, h), (256, 256));
        assert_eq!(rgba.len(), w * h * 4);
    }
}
