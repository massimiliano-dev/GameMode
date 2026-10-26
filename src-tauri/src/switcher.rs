//! Open windows, like a phone's recent apps: what Alt+Tab would show (minus
//! GameMode's own windows and the shell's), each with the library's name and
//! art when it belongs to a game or app GameMode knows, a thumbnail when
//! Windows can draw one, and the way to switch to it or close it. Games
//! frozen by Quick Resume are listed too.

// Windows draws and lists the windows; elsewhere only the logic is tested.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};

use anyhow::Result;
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::launcher::{self, Phase, SessionState};
use crate::model::{Game, LaunchSpec, Store};
use crate::{overlay, window, AppState};

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum WindowState {
    /// The game or app GameMode started and follows in the foreground.
    Playing,
    /// Frozen by Quick Resume.
    Paused,
    Open,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OpenWindow {
    /// The window handle, handed back to switch or close; 0 for a frozen
    /// game whose window can't be seen.
    pub hwnd: i64,
    pub name: String,
    /// Second line: the window's title under a library name, or the program.
    pub detail: Option<String>,
    pub game_id: Option<String>,
    pub state: WindowState,
    /// The program's icon (a local PNG).
    pub icon: Option<String>,
    /// A game's own art (hero, else cover).
    pub art: Option<String>,
    pub art_version: Option<i64>,
    pub thumbnail: Option<String>,
    /// When the thumbnail was taken (ms), so the page loads the new one.
    pub thumbnail_at: Option<i64>,
    /// Windows says it doesn't respond.
    pub hung: bool,
    /// Its process may be ended when it doesn't close by itself.
    pub can_force: bool,
}

/// A top-level window as Windows describes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawWindow {
    pub hwnd: isize,
    /// The process that owns the content (for packaged apps, the app inside
    /// the ApplicationFrameHost frame).
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub exe: Option<PathBuf>,
    pub aumid: Option<String>,
    pub minimized: bool,
    pub hung: bool,
}

impl RawWindow {
    fn exe_name(&self) -> Option<String> {
        exe_name(self.exe.as_deref()?)
    }
}

/// The last part of a path, whichever separator it uses.
fn file_part(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    text.rsplit(['\\', '/']).next().filter(|p| !p.is_empty()).map(str::to_owned)
}

fn exe_name(path: &Path) -> Option<String> {
    file_part(path).map(|n| n.to_lowercase())
}

/// Parts of Windows itself whose windows Alt+Tab skips too.
const SHELL_PROCESSES: &[&str] = &[
    "textinputhost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "searchapp.exe",
    "lockapp.exe",
    "shellhost.exe",
];
/// The desktop, the taskbars and bare shell surfaces.
const SHELL_CLASSES: &[&str] = &["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd", "Windows.UI.Core.CoreWindow"];
/// File Explorer's own windows: Explorer's other windows are the desktop.
const EXPLORER_WINDOW: &str = "CabinetWClass";
/// Never ended by force: that takes down the desktop, every packaged app,
/// or Windows itself.
const PROTECTED: &[&str] = &[
    "explorer.exe",
    "applicationframehost.exe",
    "dwm.exe",
    "csrss.exe",
    "winlogon.exe",
    "wininit.exe",
    "services.exe",
    "lsass.exe",
    "smss.exe",
    "svchost.exe",
    "sihost.exe",
    "ctfmon.exe",
    "fontdrvhost.exe",
    "runtimebroker.exe",
    "taskhostw.exe",
];
/// Thumbnails are this wide at most (physical pixels).
const THUMB_WIDTH: usize = 640;

/// Whether the switcher lists `w` at all.
fn listed(w: &RawWindow) -> bool {
    if w.title.trim().is_empty() || SHELL_CLASSES.contains(&w.class.as_str()) {
        return false;
    }
    match w.exe_name().as_deref() {
        Some(n) if SHELL_PROCESSES.contains(&n) => false,
        Some("explorer.exe") => w.class == EXPLORER_WINDOW,
        _ => true,
    }
}

/// Whether ending the process of `exe` (lowercase file name) is allowed.
fn can_force(exe: Option<&str>) -> bool {
    exe.is_some_and(|n| !PROTECTED.contains(&n))
}

/// The library entry a window belongs to: a packaged app by its AUMID, an
/// app by its executable, else the game whose folder holds the executable
/// (the deepest one, and never a bare top-level folder like `D:\Giochi`).
fn library_match<'a>(w: &RawWindow, games: &'a [Game]) -> Option<&'a Game> {
    if let Some(aumid) = w.aumid.as_deref().filter(|a| !a.is_empty()) {
        let id = Store::App.game_id(aumid);
        if let Some(game) = games.iter().find(|g| g.store == Store::App && g.id.eq_ignore_ascii_case(&id)) {
            return Some(game);
        }
    }
    let exe = launcher::normalize(w.exe.as_deref()?);
    let app = games.iter().find(|g| {
        g.store == Store::App && matches!(&g.launch, LaunchSpec::Exe { path, .. } if launcher::normalize(path) == exe)
    });
    app.or_else(|| {
        games
            .iter()
            .filter(|g| g.store != Store::App)
            .filter_map(|g| Some((g, launcher::normalize(g.install_dir.as_deref()?))))
            .filter(|(_, dir)| dir.matches('/').count() >= 2 && exe.starts_with(&format!("{dir}/")))
            .max_by_key(|(_, dir)| dir.len())
            .map(|(g, _)| g)
    })
}

/// "chrome.exe" → "Chrome": what to call a program the library doesn't know.
fn program_name(exe: &Path) -> Option<String> {
    let file = file_part(exe)?;
    let stem = file.rsplit_once('.').map_or(file.as_str(), |(stem, _)| stem);
    let mut chars = stem.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// Windows' windows as the switcher lists them, front to back: one entry per
/// followed game (its frontmost window), the library's name and art for the
/// programs it knows, then the frozen games no window shows.
fn build(raws: &[RawWindow], sessions: &[(SessionState, Vec<u32>)], games: &[Game], own_exe: Option<&Path>) -> Vec<(OpenWindow, RawWindow)> {
    let own = own_exe.map(launcher::normalize);
    let mut out: Vec<(OpenWindow, RawWindow)> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for raw in raws.iter().filter(|w| listed(w)) {
        if own.is_some() && raw.exe.as_deref().map(launcher::normalize) == own {
            continue;
        }
        let session = sessions.iter().find(|(_, pids)| pids.contains(&raw.pid)).map(|(s, _)| s);
        if let Some(s) = session {
            if seen.contains(&s.game_id.as_str()) {
                continue;
            }
            seen.push(&s.game_id);
        }
        let game = session.and_then(|s| games.iter().find(|g| g.id == s.game_id)).or_else(|| library_match(raw, games));
        let state = match session {
            Some(s) if s.phase == Phase::Suspended => WindowState::Paused,
            Some(_) => WindowState::Playing,
            None => WindowState::Open,
        };
        let name = game.map(|g| g.name.clone()).or_else(|| session.map(|s| s.game_name.clone())).unwrap_or_else(|| raw.title.clone());
        let detail = if name != raw.title { Some(raw.title.clone()) } else { raw.exe.as_deref().and_then(program_name) };
        let mut entry = entry(raw.hwnd as i64, name, detail, game, session.map(|s| s.game_id.clone()), state);
        entry.hung = raw.hung;
        entry.can_force = state != WindowState::Paused && can_force(raw.exe_name().as_deref());
        out.push((entry, raw.clone()));
    }
    for s in sessions.iter().map(|(s, _)| s).filter(|s| s.phase == Phase::Suspended && !seen.contains(&s.game_id.as_str())) {
        let game = games.iter().find(|g| g.id == s.game_id);
        let name = game.map_or_else(|| s.game_name.clone(), |g| g.name.clone());
        out.push((entry(0, name, None, game, Some(s.game_id.clone()), WindowState::Paused), RawWindow::default()));
    }
    out
}

fn entry(hwnd: i64, name: String, detail: Option<String>, game: Option<&Game>, session: Option<String>, state: WindowState) -> OpenWindow {
    let is_app = game.is_some_and(|g| g.store == Store::App);
    OpenWindow {
        hwnd,
        name,
        detail,
        game_id: session.or_else(|| game.map(|g| g.id.clone())),
        state,
        // An app's "cover" is its icon; a game's art is its picture.
        icon: game.filter(|_| is_app).and_then(|g| g.artwork.cover.clone()),
        art: game.filter(|_| !is_app).and_then(|g| g.artwork.hero.clone().or_else(|| g.artwork.cover.clone())),
        art_version: game.and_then(|g| g.artwork_checked_at),
        thumbnail: None,
        thumbnail_at: None,
        hung: false,
        can_force: false,
    }
}

/// Shrinks the `crop` (x, y, width, height) of a top-down BGRA image
/// `width` pixels wide to at most `max_w` pixels across, averaging, as
/// opaque RGBA. `None` when the crop is empty or the picture is all black
/// (what Windows hands back for windows it can't draw).
fn downscale(bgra: &[u8], width: usize, crop: (usize, usize, usize, usize), max_w: usize) -> Option<(u32, u32, Vec<u8>)> {
    let (cx, cy, cw, ch) = crop;
    if cw == 0 || ch == 0 || width == 0 || (cy + ch) * width * 4 > bgra.len() || cx + cw > width {
        return None;
    }
    let tw = cw.min(max_w).max(1);
    let th = (ch * tw / cw).max(1);
    let mut out = Vec::with_capacity(tw * th * 4);
    let mut brightest = 0u8;
    for ty in 0..th {
        let (y0, y1) = (cy + ty * ch / th, (cy + (ty + 1) * ch / th).max(cy + ty * ch / th + 1));
        for tx in 0..tw {
            let (x0, x1) = (cx + tx * cw / tw, (cx + (tx + 1) * cw / tw).max(cx + tx * cw / tw + 1));
            let mut sum = [0u32; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = (y * width + x) * 4;
                    sum[0] += u32::from(bgra[p + 2]);
                    sum[1] += u32::from(bgra[p + 1]);
                    sum[2] += u32::from(bgra[p]);
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let px = [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8];
            brightest = brightest.max(px[0]).max(px[1]).max(px[2]);
            out.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
    }
    (brightest >= 12).then_some((tw as u32, th as u32, out))
}

/// The window the user picked last. In console mode GameMode takes the
/// screen back from pushy background tools: not from this one, which the
/// user asked for.
static PICKED: AtomicIsize = AtomicIsize::new(0);

pub fn picked(hwnd: isize) -> bool {
    hwnd != 0 && PICKED.load(Ordering::SeqCst) == hwnd
}

/// The open windows, front to back. `thumbnails`: also draw each one
/// (slower: the page asks without them first). Blocking.
pub fn list(app: &AppHandle, thumbnails: bool) -> Vec<OpenWindow> {
    let state = app.state::<AppState>();
    let sessions = state.launcher.followed();
    let games = state.db.lock().view();
    let own = std::env::current_exe().ok();
    #[cfg(windows)]
    let raws = imp::windows();
    #[cfg(not(windows))]
    let raws: Vec<RawWindow> = Vec::new();
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut entries = build(&raws, &sessions, &games, own.as_deref());
    #[cfg(windows)]
    imp::decorate(&state.data_dir, &mut entries, thumbnails);
    #[cfg(not(windows))]
    let _ = thumbnails;
    entries.into_iter().map(|(w, _)| w).collect()
}

fn is_suspended(app: &AppHandle, game_id: Option<&str>) -> Option<String> {
    let id = game_id?;
    app.state::<AppState>().launcher.suspended().into_iter().find(|s| s.game_id == id).map(|s| s.game_id)
}

/// Leaves the in-game overlay before another window takes the screen: the
/// game it paused runs again.
fn leave_overlay(app: &AppHandle) {
    if overlay::is_open() {
        overlay::hide(app);
        crate::overlay_pause::thaw_now();
    }
}

/// Puts a window in front: a frozen game is resumed (Quick Resume), any
/// other window is restored and gets the screen. Blocking.
pub fn switch_to(app: &AppHandle, hwnd: isize, game_id: Option<&str>) -> Result<()> {
    if let Some(id) = is_suspended(app, game_id) {
        leave_overlay(app);
        return launcher::resume_suspended(app, &id);
    }
    anyhow::ensure!(hwnd != 0 && window::window_alive(hwnd), "Questa finestra non c'è più");
    leave_overlay(app);
    crate::assist::disable();
    PICKED.store(hwnd, Ordering::SeqCst);
    window::force_foreground_raw(hwnd);
    window::minimize(app);
    Ok(())
}

/// Closes a window like its X button (the app may save, or ask), or with
/// `force` ends its process. A frozen game is closed through Quick Resume,
/// the game GameMode follows through its session. Blocking.
pub fn close(app: &AppHandle, hwnd: isize, game_id: Option<&str>, force: bool) -> Result<()> {
    if let Some(id) = is_suspended(app, game_id) {
        return launcher::close_suspended(app, &id);
    }
    let current = app.state::<AppState>().launcher.current();
    if force && game_id.is_some() && current.is_some_and(|s| Some(s.game_id.as_str()) == game_id) {
        launcher::close_running(app);
        return Ok(());
    }
    anyhow::ensure!(hwnd != 0 && window::window_alive(hwnd), "Questa finestra non c'è più");
    #[cfg(windows)]
    {
        if force {
            imp::end_process(hwnd)
        } else {
            imp::post_close(hwnd)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = force;
        anyhow::bail!("Solo su Windows")
    }
}

/// Asks every open window to close (frozen games end). Returns how many
/// took the request. Blocking.
pub fn close_all(app: &AppHandle) -> usize {
    list(app, false)
        .iter()
        .filter(|w| close(app, w.hwnd as isize, w.game_id.as_deref(), false).is_ok())
        .count()
}

#[cfg(windows)]
mod imp {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use anyhow::{bail, ensure, Context, Result};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW,
        GetWindowTextW, GetWindowThreadProcessId, IsHungAppWindow, IsIconic, IsWindowVisible, PostMessageW, GWL_EXSTYLE,
        GW_OWNER, PW_RENDERFULLCONTENT, WM_CLOSE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };

    use super::{can_force, downscale, exe_name, OpenWindow, RawWindow, WindowState, THUMB_WIDTH};

    /// How long all thumbnails together may take: a window that doesn't
    /// draw in time just shows its icon.
    const THUMB_BUDGET: Duration = Duration::from_millis(1500);

    fn hwnd(raw: isize) -> HWND {
        HWND(raw as *mut core::ffi::c_void)
    }

    /// Every top-level window, front to back, the way Alt+Tab picks them:
    /// visible, not cloaked, not a tool window or an owned popup, not
    /// GameMode's.
    pub fn windows() -> Vec<RawWindow> {
        unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let found = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
            found.push(hwnd);
            BOOL(1)
        }
        let mut all: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut Vec<HWND> as isize));
        }
        let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let me = std::process::id();
        let out = all.into_iter().filter_map(|h| describe(h, me)).collect();
        if init.is_ok() {
            unsafe { CoUninitialize() };
        }
        out
    }

    fn describe(window: HWND, me: u32) -> Option<RawWindow> {
        unsafe {
            if !IsWindowVisible(window).as_bool() {
                return None;
            }
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(window, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
            if cloaked != 0 {
                return None;
            }
            let ex = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
            let app_window = ex & WS_EX_APPWINDOW.0 != 0;
            let owned = GetWindow(window, GW_OWNER).is_ok_and(|o| !o.is_invalid());
            if !app_window && (ex & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0 || owned) {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if pid == me {
                return None;
            }
            let title = title(window);
            if title.trim().is_empty() {
                return None;
            }
            let class = class_name(window);
            let pid = if class == "ApplicationFrameWindow" { hosted_pid(window, pid).unwrap_or(pid) } else { pid };
            Some(RawWindow {
                hwnd: window.0 as isize,
                pid,
                title,
                class,
                exe: crate::window::process_path(pid),
                aumid: aumid(window),
                minimized: IsIconic(window).as_bool(),
                hung: IsHungAppWindow(window).as_bool(),
            })
        }
    }

    fn title(window: HWND) -> String {
        unsafe {
            let len = GetWindowTextLengthW(window);
            if len <= 0 {
                return String::new();
            }
            let mut buf = vec![0u16; len as usize + 1];
            let got = GetWindowTextW(window, &mut buf).max(0) as usize;
            String::from_utf16_lossy(&buf[..got])
        }
    }

    fn class_name(window: HWND) -> String {
        let mut buf = [0u16; 256];
        let len = unsafe { GetClassNameW(window, &mut buf) }.max(0) as usize;
        String::from_utf16_lossy(&buf[..len])
    }

    /// The app inside a packaged app's frame: the child window from another process.
    fn hosted_pid(frame: HWND, frame_pid: u32) -> Option<u32> {
        struct Search {
            frame_pid: u32,
            found: Option<u32>,
        }
        unsafe extern "system" fn visit(child: HWND, lparam: LPARAM) -> BOOL {
            let search = unsafe { &mut *(lparam.0 as *mut Search) };
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(child, Some(&mut pid)) };
            if pid != 0 && pid != search.frame_pid {
                search.found = Some(pid);
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut search = Search { frame_pid, found: None };
        unsafe {
            let _ = EnumChildWindows(Some(frame), Some(visit), LPARAM(&mut search as *mut Search as isize));
        }
        search.found
    }

    /// The AppUserModelID a window declares (packaged apps always do).
    fn aumid(window: HWND) -> Option<String> {
        unsafe {
            let store: IPropertyStore = SHGetPropertyStoreForWindow(window).ok()?;
            let value = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
            let text = PropVariantToStringAlloc(&value).ok()?;
            if text.is_null() {
                return None;
            }
            let s = text.to_string().unwrap_or_default();
            CoTaskMemFree(Some(text.0 as *const _));
            (!s.is_empty()).then_some(s)
        }
    }

    /// Icons the shell couldn't give: not asked again in this run.
    static NO_ICON: Mutex<Option<HashSet<String>>> = Mutex::new(None);

    /// Fills in icons (cached next to the apps' ones) and, with
    /// `thumbnails`, a fresh picture of every window that can be drawn.
    pub fn decorate(data_dir: &Path, entries: &mut [(OpenWindow, RawWindow)], thumbnails: bool) {
        let icon_dir = crate::library::apps::icon_dir(data_dir);
        for (entry, raw) in entries.iter_mut().filter(|(e, _)| e.icon.is_none()) {
            entry.icon = icon(&icon_dir, raw).map(|p| p.to_string_lossy().into_owned());
        }
        if thumbnails {
            take_thumbnails(&data_dir.join("artwork").join("switcher"), entries);
        }
    }

    fn icon(dir: &Path, raw: &RawWindow) -> Option<PathBuf> {
        let source = match (raw.aumid.as_deref(), raw.exe.as_deref()) {
            (Some(aumid), _) => format!("shell:AppsFolder\\{aumid}"),
            (None, Some(exe)) => exe.to_string_lossy().into_owned(),
            (None, None) => return None,
        };
        let file = crate::library::apps::icon_file(dir, &format!("window:{source}"));
        if file.is_file() {
            return Some(file);
        }
        let mut failed = NO_ICON.lock().unwrap_or_else(|e| e.into_inner());
        if failed.get_or_insert_with(HashSet::new).contains(&source) {
            return None;
        }
        match crate::library::apps::save_item_icon(&source, &file) {
            Ok(()) => Some(file),
            Err(_) => {
                failed.get_or_insert_with(HashSet::new).insert(source);
                None
            }
        }
    }

    /// Draws the windows in parallel within `THUMB_BUDGET`. Minimized,
    /// frozen and hung windows are skipped: they can't draw (a frozen one
    /// would never answer). Old pictures of closed windows go.
    fn take_thumbnails(dir: &Path, entries: &mut [(OpenWindow, RawWindow)]) {
        let _ = std::fs::create_dir_all(dir);
        let mut pending = Vec::new();
        for (i, (entry, raw)) in entries.iter().enumerate() {
            if raw.hwnd == 0 || raw.minimized || raw.hung || entry.state == WindowState::Paused {
                continue;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let out = dir.join(format!("{:x}.png", raw.hwnd));
            let target = raw.hwnd;
            std::thread::spawn(move || {
                let _ = tx.send(thumbnail(target, &out).map(|()| out));
            });
            pending.push((i, rx));
        }
        let deadline = Instant::now() + THUMB_BUDGET;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
        let mut keep: HashSet<PathBuf> = HashSet::new();
        for (i, rx) in pending {
            if let Ok(Ok(path)) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                entries[i].0.thumbnail = Some(path.to_string_lossy().into_owned());
                entries[i].0.thumbnail_at = Some(stamp);
                keep.insert(path);
            }
        }
        if let Ok(files) = std::fs::read_dir(dir) {
            for file in files.flatten().map(|f| f.path()) {
                if !keep.contains(&file) && file.extension().is_some_and(|e| e == "png") {
                    let _ = std::fs::remove_file(file);
                }
            }
        }
    }

    /// A picture of one window as DWM composes it (also hardware-drawn
    /// content), cropped to its visible frame and scaled down.
    fn thumbnail(raw: isize, out: &Path) -> Result<()> {
        unsafe {
            let window = hwnd(raw);
            let mut rect = RECT::default();
            GetWindowRect(window, &mut rect)?;
            let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
            ensure!(width > 0 && height > 0 && width <= 8192 && height <= 8192, "window size {width}x{height}");
            // Maximized and themed windows extend past what's on screen.
            let mut frame = RECT::default();
            let framed = DwmGetWindowAttribute(
                window,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                &mut frame as *mut RECT as *mut _,
                std::mem::size_of::<RECT>() as u32,
            )
            .is_ok();
            let crop = if framed {
                let x = (frame.left - rect.left).clamp(0, width - 1);
                let y = (frame.top - rect.top).clamp(0, height - 1);
                let w = (frame.right - frame.left).clamp(1, width - x);
                let h = (frame.bottom - frame.top).clamp(1, height - y);
                (x as usize, y as usize, w as usize, h as usize)
            } else {
                (0, 0, width as usize, height as usize)
            };

            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            let previous = SelectObject(mem, HGDIOBJ(bitmap.0));
            let printed = PrintWindow(window, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
            SelectObject(mem, previous);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bgra = vec![0u8; (width * height * 4) as usize];
            let lines = if printed {
                GetDIBits(mem, bitmap, 0, height as u32, Some(bgra.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS)
            } else {
                0
            };
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            ensure!(lines == height, "PrintWindow copied {lines} of {height} lines");
            let (w, h, rgba) = downscale(&bgra, width as usize, crop, THUMB_WIDTH).context("nothing drawn")?;
            crate::library::apps::write_png(out, w, h, &rgba)
        }
    }

    /// Like the window's X button: the app may save first, or ask.
    pub fn post_close(raw: isize) -> Result<()> {
        unsafe { PostMessageW(Some(hwnd(raw)), WM_CLOSE, WPARAM(0), LPARAM(0)) }
            .context("Windows non lascia chiudere questa finestra: forse l'app è avviata come amministratore")
    }

    /// Ends the process behind a window that doesn't close.
    pub fn end_process(raw: isize) -> Result<()> {
        let window = hwnd(raw);
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        if class_name(window) == "ApplicationFrameWindow" {
            pid = hosted_pid(window, pid).unwrap_or(pid);
        }
        ensure!(pid != 0 && pid != std::process::id(), "Questa finestra non c'è più");
        let exe = crate::window::process_path(pid).and_then(|p| exe_name(&p));
        if !can_force(exe.as_deref()) {
            bail!("Questa finestra fa parte di Windows: non si può forzare");
        }
        ensure!(
            crate::launcher::kill(&[pid]),
            "Windows non lascia chiudere questa app: forse è avviata come amministratore"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(hwnd: isize, pid: u32, title: &str, exe: &str) -> RawWindow {
        RawWindow { hwnd, pid, title: title.into(), class: "Chrome_WidgetWin_1".into(), exe: Some(PathBuf::from(exe)), ..Default::default() }
    }

    fn session(id: &str, phase: Phase) -> SessionState {
        SessionState {
            game_id: id.into(),
            game_name: "Hades".into(),
            phase,
            started_at: 0,
            seconds: 0,
            update: None,
            suspended_at: None,
            anti_cheat: false,
            stage: None,
        }
    }

    #[test]
    fn lists_what_alt_tab_shows() {
        assert!(listed(&raw(1, 1, "Google Chrome", r"C:\Chrome\chrome.exe")));
        assert!(!listed(&raw(1, 1, "  ", r"C:\Chrome\chrome.exe")));
        assert!(!listed(&raw(1, 1, "Ricerca", r"C:\Windows\SystemApps\SearchHost.exe")));
        let mut desktop = raw(1, 1, "Program Manager", r"C:\Windows\explorer.exe");
        desktop.class = "Progman".into();
        assert!(!listed(&desktop));
        let mut files = raw(1, 1, "Download", r"C:\Windows\explorer.exe");
        files.class = "CabinetWClass".into();
        assert!(listed(&files));
        files.class = "SomethingElse".into();
        assert!(!listed(&files));
    }

    #[test]
    fn never_forces_windows_own_processes() {
        assert!(can_force(Some("chrome.exe")));
        assert!(!can_force(Some("explorer.exe")));
        assert!(!can_force(Some("applicationframehost.exe")));
        assert!(!can_force(None));
    }

    #[test]
    fn names_windows_after_the_library() {
        let mut notepad = Game::new(Store::App, "notepad", "Blocco note", LaunchSpec::Exe { path: r"C:\Windows\notepad.exe".into(), args: vec![], cwd: None });
        notepad.artwork.cover = Some("icon.png".into());
        let mut witcher = Game::new(Store::Gog, "1207664643", "The Witcher 3", LaunchSpec::Uri { uri: String::new() });
        witcher.install_dir = Some(r"D:\Giochi\The Witcher 3".into());
        witcher.artwork.hero = Some("hero.jpg".into());
        let mut broad = Game::new(Store::Manual, "x", "Troppo largo", LaunchSpec::Uri { uri: String::new() });
        broad.install_dir = Some(r"D:\Giochi".into());
        let games = [notepad, witcher, broad];

        let windows = [
            raw(1, 10, "Senza titolo - Blocco note", r"C:\Windows\NOTEPAD.EXE"),
            raw(2, 11, "The Witcher 3", r"D:\Giochi\The Witcher 3\bin\x64\witcher3.exe"),
            raw(3, 12, "Calcolatrice", r"D:\Giochi\Altro\calc.exe"),
            raw(4, 13, "GameMode", r"C:\Users\max\AppData\Local\GameMode\gamemode.exe"),
        ];
        let list = build(&windows, &[], &games, Some(Path::new(r"C:\Users\max\AppData\Local\GameMode\gamemode.exe")));
        assert_eq!(list.len(), 3, "GameMode itself is left out");

        let (pad, _) = &list[0];
        assert_eq!(pad.name, "Blocco note");
        assert_eq!(pad.detail.as_deref(), Some("Senza titolo - Blocco note"));
        assert_eq!(pad.icon.as_deref(), Some("icon.png"));
        assert_eq!(pad.art, None);
        assert_eq!(pad.state, WindowState::Open);
        assert!(pad.can_force);

        let (game, _) = &list[1];
        assert_eq!(game.game_id.as_deref(), Some("gog:1207664643"));
        assert_eq!(game.art.as_deref(), Some("hero.jpg"));
        assert_eq!(game.detail.as_deref(), Some("Witcher3"), "same title: the program goes underneath");

        // A bare top-level folder isn't a game's.
        let (other, _) = &list[2];
        assert_eq!(other.game_id, None);
        assert_eq!(other.name, "Calcolatrice");
    }

    #[test]
    fn one_entry_per_game_and_frozen_games_too() {
        let windows = [raw(1, 7, "Hades", r"D:\Hades\Hades.exe"), raw(2, 7, "Hades launcher", r"D:\Hades\Hades.exe")];
        let sessions = [
            (session("steam:1145360", Phase::Running), vec![7]),
            (session("steam:570", Phase::Suspended), vec![99]),
        ];
        let list = build(&windows, &sessions, &[], None);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].0.hwnd, 1);
        assert_eq!(list[0].0.state, WindowState::Playing);
        assert_eq!(list[0].0.game_id.as_deref(), Some("steam:1145360"));
        assert_eq!(list[1].0.hwnd, 0);
        assert_eq!(list[1].0.state, WindowState::Paused);
        assert!(!list[1].0.can_force, "frozen games close through Quick Resume");
    }

    #[test]
    fn packaged_apps_match_by_aumid() {
        let store = Game::new(Store::App, "Microsoft.WindowsStore_8wekyb3d8bbwe!App", "Microsoft Store", LaunchSpec::AppsFolder { aumid: "Microsoft.WindowsStore_8wekyb3d8bbwe!App".into() });
        let mut w = raw(5, 3, "Microsoft Store", r"C:\Program Files\WindowsApps\Store\WinStore.App.exe");
        w.aumid = Some("microsoft.windowsstore_8wekyb3d8bbwe!App".into());
        assert_eq!(library_match(&w, std::slice::from_ref(&store)).map(|g| g.name.as_str()), Some("Microsoft Store"));
    }

    #[test]
    fn scales_down_and_drops_black_pictures() {
        // 4x2 BGRA, cropped to the right 2x2 half: red on top, blue below.
        let mut bgra = vec![0u8; 4 * 2 * 4];
        for (x, y, b, r) in [(2, 0, 0, 200), (3, 0, 0, 200), (2, 1, 200, 0), (3, 1, 200, 0)] {
            let p = (y * 4 + x) * 4;
            bgra[p] = b;
            bgra[p + 2] = r;
        }
        let (w, h, rgba) = downscale(&bgra, 4, (2, 0, 2, 2), 1).unwrap();
        assert_eq!((w, h), (1, 1));
        assert_eq!(rgba, vec![100, 0, 100, 255]);
        let (w, h, _) = downscale(&bgra, 4, (2, 0, 2, 2), 640).unwrap();
        assert_eq!((w, h), (2, 2), "never scaled up");
        assert_eq!(downscale(&vec![0u8; 64], 4, (0, 0, 4, 4), 640), None, "all black");
        assert_eq!(downscale(&bgra, 4, (3, 0, 2, 2), 640), None, "crop outside the picture");
    }

    #[test]
    fn programs_get_readable_names() {
        assert_eq!(program_name(Path::new(r"C:\x\chrome.exe")).as_deref(), Some("Chrome"));
        assert_eq!(program_name(Path::new("")), None);
    }
}
