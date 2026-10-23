//! Keeping the launcher on screen: Windows refuses `SetForegroundWindow` from
//! background processes, so returning from a game needs a few tricks.

use tauri::{AppHandle, Manager, WebviewWindow};

use crate::AppState;

pub fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
}

/// Fullscreen is skipped in debug builds so development stays usable.
pub fn wants_fullscreen(app: &AppHandle) -> bool {
    !cfg!(debug_assertions) && app.state::<AppState>().db.lock().settings.start_fullscreen
}

/// Applies the TV display mode and audio output, when configured.
pub fn apply_tv(app: &AppHandle) {
    let settings = app.state::<AppState>().db.lock().settings.clone();
    // Safe mode: a display mode the TV can't show may be what went wrong.
    if wants_fullscreen(app) && !crate::shell::safe_mode() {
        crate::display::apply_tv(settings.tv_mode, settings.tv_audio.as_deref());
    }
}

pub fn bring_to_front(app: &AppHandle) {
    crate::shell::console_in_front();
    apply_tv(app);
    let Some(window) = main_window(app) else { return };
    let _ = window.unminimize();
    let _ = window.show();
    if wants_fullscreen(app) {
        let _ = window.set_fullscreen(true);
    }
    let _ = window.set_focus();
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd() {
        win::force_foreground(win::hwnd(hwnd.0 as isize));
    }
}

pub fn minimize(app: &AppHandle) {
    // In console mode there's no desktop underneath: stepping aside would
    // leave a black screen if the other app doesn't show up.
    if crate::shell::no_desktop() {
        return;
    }
    if let Some(window) = main_window(app) {
        let _ = window.minimize();
    }
}

pub fn exit_to_desktop(app: &AppHandle) {
    crate::display::reset_mode();
    if let Some(window) = main_window(app) {
        let _ = window.set_fullscreen(false);
        let _ = window.minimize();
    }
}

pub fn toggle_fullscreen(app: &AppHandle) {
    if let Some(window) = main_window(app) {
        let on = window.is_fullscreen().unwrap_or(false);
        let _ = window.set_fullscreen(!on);
    }
}

pub fn is_foreground(app: &AppHandle) -> bool {
    let Some(window) = main_window(app) else { return false };
    #[cfg(windows)]
    {
        window.hwnd().map(|h| win::is_foreground(win::hwnd(h.0 as isize))).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        window.is_focused().unwrap_or(false)
    }
}

pub fn is_hwnd_foreground(raw: isize) -> bool {
    #[cfg(windows)]
    {
        win::is_foreground(win::hwnd(raw))
    }
    #[cfg(not(windows))]
    {
        let _ = raw;
        true
    }
}

pub fn force_foreground_raw(raw: isize) {
    #[cfg(windows)]
    win::force_foreground(win::hwnd(raw));
    #[cfg(not(windows))]
    let _ = raw;
}

/// Executable name (lowercase, e.g. "steam.exe") of the foreground window's process.
#[cfg(windows)]
pub fn foreground_process_name() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut pid));
    }
    process_name(pid)
}

/// Visible, real-sized top-level windows of the processes named in `names`
/// (lowercase exe names).
pub fn windows_of(names: &[&str]) -> Vec<isize> {
    #[cfg(windows)]
    {
        let mut cache = std::collections::HashMap::new();
        win::visible_windows()
            .into_iter()
            .filter(|(_, pid)| {
                cache
                    .entry(*pid)
                    .or_insert_with(|| process_name(*pid))
                    .as_deref()
                    .is_some_and(|n| names.contains(&n))
            })
            .map(|(hwnd, _)| hwnd)
            .collect()
    }
    #[cfg(not(windows))]
    {
        let _ = names;
        Vec::new()
    }
}

#[cfg(windows)]
fn process_name(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(handle);
        ok.then(|| {
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            path.rsplit(['\\', '/']).next().unwrap_or_default().to_lowercase()
        })
    }
}

/// Minimizes the foreground window if it belongs to one of `processes`
/// (lowercase exe names). Returns whether it did.
#[cfg(windows)]
pub fn minimize_foreground_of(processes: &[&str]) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, ShowWindowAsync, SW_MINIMIZE};
    if !foreground_process_name().is_some_and(|n| processes.contains(&n.as_str())) {
        return false;
    }
    unsafe { ShowWindowAsync(GetForegroundWindow(), SW_MINIMIZE).as_bool() }
}

/// Puts a window back on top of other always-on-top windows (Steam, Game
/// Bar) without taking focus.
#[cfg(windows)]
pub fn raise_topmost_raw(raw: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    };
    unsafe {
        let _ = SetWindowPos(win::hwnd(raw), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

/// Brings the top-level window of one of `pids` to the front (the running game).
pub fn is_minimized(app: &AppHandle) -> bool {
    main_window(app).and_then(|w| w.is_minimized().ok()).unwrap_or(false)
}

/// The main window of a game's processes (the largest visible one).
pub fn game_window(pids: &[u32]) -> Option<isize> {
    #[cfg(windows)]
    {
        win::find_window(pids).map(|h| h.0 as isize)
    }
    #[cfg(not(windows))]
    {
        // No window lookup here (dev on macOS): the process stands in.
        pids.first().map(|&p| p as isize)
    }
}

/// Nothing to look at in front: no foreground window, a hidden one, or
/// Explorer's desktop and taskbar (hidden in console mode).
pub fn nothing_in_front() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, IsWindowVisible};
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() || !IsWindowVisible(hwnd).as_bool() {
                return true;
            }
            let mut class = [0u16; 64];
            let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..len]);
            matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd")
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Moves another app's window to the screen GameMode is on (apps like Wand
/// remember the second monitor and open over there): maximized, or centered
/// at its own size for dialogs.
pub fn move_to_gamemode_screen(app: &AppHandle, raw: isize, maximize: bool) {
    #[cfg(windows)]
    {
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowRect, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_MAXIMIZE, SW_RESTORE,
        };
        let Some(main) = main_window(app).and_then(|w| w.hwnd().ok()) else { return };
        let target = win::hwnd(raw);
        unsafe {
            // A minimized GameMode still counts where it was before.
            let screen = MonitorFromWindow(win::hwnd(main.0 as isize), MONITOR_DEFAULTTOPRIMARY);
            if MonitorFromWindow(target, MONITOR_DEFAULTTOPRIMARY) == screen {
                return;
            }
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(screen, &mut info).as_bool() {
                return;
            }
            let work = info.rcWork;
            let _ = ShowWindow(target, SW_RESTORE);
            if maximize {
                let (w, h) = (work.right - work.left, work.bottom - work.top);
                let _ = SetWindowPos(target, None, work.left, work.top, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
                let _ = ShowWindow(target, SW_MAXIMIZE);
            } else {
                let mut rect = RECT::default();
                let _ = GetWindowRect(target, &mut rect);
                let x = work.left + ((work.right - work.left) - (rect.right - rect.left)).max(0) / 2;
                let y = work.top + ((work.bottom - work.top) - (rect.bottom - rect.top)).max(0) / 2;
                let _ = SetWindowPos(target, None, x, y, 0, 0, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (app, raw, maximize);
    }
}

/// The window still exists and is shown.
pub fn window_alive(raw: isize) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{IsWindow, IsWindowVisible};
        let hwnd = win::hwnd(raw);
        unsafe { IsWindow(Some(hwnd)).as_bool() && IsWindowVisible(hwnd).as_bool() }
    }
    #[cfg(not(windows))]
    {
        let _ = raw;
        false
    }
}

/// Another app covers the whole screen in front (most likely a game GameMode
/// lost track of): GameMode must not jump over it.
pub fn fullscreen_app_in_front(app: &AppHandle) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};
        if is_foreground(app) || crate::overlay::is_open() {
            return false;
        }
        if foreground_process_name().is_none_or(|n| n == "explorer.exe") {
            return false;
        }
        unsafe {
            let hwnd = GetForegroundWindow();
            let mut rect = RECT::default();
            let mut monitor = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetWindowRect(hwnd, &mut rect).is_err()
                || !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut monitor).as_bool()
            {
                return false;
            }
            let screen = monitor.rcMonitor;
            rect.left <= screen.left && rect.top <= screen.top && rect.right >= screen.right && rect.bottom >= screen.bottom
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        false
    }
}

/// Brings the running game back and gets GameMode out of its way.
pub fn resume_game(app: &AppHandle, pids: &[u32]) -> bool {
    let found = focus_process(pids);
    if found {
        minimize(app);
    }
    found
}

pub fn focus_process(pids: &[u32]) -> bool {
    #[cfg(windows)]
    {
        match win::find_window(pids) {
            Some(hwnd) => {
                win::force_foreground(hwnd);
                true
            }
            None => false,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pids;
        false
    }
}

#[cfg(windows)]
pub(crate) mod win {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindow, GetWindowRect,
        GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GW_OWNER,
        SW_RESTORE, SW_SHOW,
    };

    pub fn hwnd(raw: isize) -> HWND {
        HWND(raw as *mut core::ffi::c_void)
    }

    pub fn is_foreground(hwnd: HWND) -> bool {
        unsafe { GetForegroundWindow() == hwnd }
    }

    pub fn force_foreground(hwnd: HWND) {
        unsafe {
            let _ = ShowWindow(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
            if SetForegroundWindow(hwnd).as_bool() && is_foreground(hwnd) {
                return;
            }
            // Borrow the foreground thread's input state, which lifts the
            // foreground lock for this call.
            let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
            let me = GetCurrentThreadId();
            let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            if attached {
                let _ = AttachThreadInput(me, fg_thread, false);
            }
            if !is_foreground(hwnd) {
                // Last resort: a synthetic Alt tap counts as user input.
                keybd_event(VK_MENU.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
                keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }

    struct Search<'a> {
        pids: &'a [u32],
        found: Option<HWND>,
        area: i64,
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let top_level = GetWindow(hwnd, GW_OWNER).map(|o| o.is_invalid()).unwrap_or(true);
            if search.pids.contains(&pid) && IsWindowVisible(hwnd).as_bool() && top_level {
                // Prefer the game itself over splash screens and launchers.
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                let area = i64::from(rect.right - rect.left) * i64::from(rect.bottom - rect.top);
                if search.found.is_none() || area > search.area {
                    search.found = Some(hwnd);
                    search.area = area;
                }
            }
        }
        BOOL(1)
    }

    /// Visible, uncloaked top-level windows of a usable size, with their pid.
    pub fn visible_windows() -> Vec<(isize, u32)> {
        unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let found = unsafe { &mut *(lparam.0 as *mut Vec<(isize, u32)>) };
            unsafe {
                if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                    return BOOL(1);
                }
                let mut cloaked = 0u32;
                let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                if cloaked != 0 || rect.right - rect.left < 240 || rect.bottom - rect.top < 120 {
                    return BOOL(1);
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                found.push((hwnd.0 as isize, pid));
            }
            BOOL(1)
        }
        let mut found: Vec<(isize, u32)> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut Vec<(isize, u32)> as isize));
        }
        found
    }

    pub fn find_window(pids: &[u32]) -> Option<HWND> {
        let mut search = Search { pids, found: None, area: 0 };
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
        }
        search.found
    }
}
