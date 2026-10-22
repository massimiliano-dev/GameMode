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
    if wants_fullscreen(app) {
        crate::display::apply_tv(settings.tv_mode, settings.tv_audio.as_deref());
    }
}

pub fn bring_to_front(app: &AppHandle) {
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

#[cfg(windows)]
pub fn is_hwnd_foreground(raw: isize) -> bool {
    win::is_foreground(win::hwnd(raw))
}

#[cfg(windows)]
pub fn force_foreground_raw(raw: isize) {
    win::force_foreground(win::hwnd(raw));
}

/// Brings the top-level window of one of `pids` to the front (the running game).
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
mod win {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindow,
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
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let top_level = GetWindow(hwnd, GW_OWNER).map(|o| o.is_invalid()).unwrap_or(true);
            if search.pids.contains(&pid) && IsWindowVisible(hwnd).as_bool() && top_level {
                search.found = Some(hwnd);
                return BOOL(0);
            }
        }
        BOOL(1)
    }

    pub fn find_window(pids: &[u32]) -> Option<HWND> {
        let mut search = Search { pids, found: None };
        unsafe {
            // Returns an error when the callback stops early; that's expected.
            let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
        }
        search.found
    }
}
