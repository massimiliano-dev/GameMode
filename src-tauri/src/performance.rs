//! Performance mode while a game runs: the "High performance" power plan and
//! politely closing apps the user listed (browsers, chat apps…). Everything
//! is undone when the game ends. The previous power plan is remembered in
//! the live-session file, so it comes back even after a crash.

const HIGH_PERFORMANCE: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";

pub fn begin(close_apps: &[String]) {
    if let Some(current) = active_scheme() {
        if !current.eq_ignore_ascii_case(HIGH_PERFORMANCE) && powercfg(&["/setactive", HIGH_PERFORMANCE]) {
            crate::live::set_power_scheme(Some(current));
        }
    }
    close_windows_of(close_apps);
}

/// Restores the previous power plan, if one was changed (also by a crashed
/// GameMode, whose plan survives in the live-session file).
pub fn end() {
    if let Some(previous) = crate::live::take_power_scheme() {
        powercfg(&["/setactive", &previous]);
    }
}

/// The GUID in `powercfg /getactivescheme` output, whatever the language.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn parse_scheme(output: &str) -> Option<String> {
    output.split_whitespace().find(|w| w.len() == 36 && w.matches('-').count() == 4).map(String::from)
}

#[cfg(windows)]
fn command(args: &[&str]) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = std::process::Command::new("powercfg");
    cmd.args(args).creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(windows)]
fn active_scheme() -> Option<String> {
    let out = command(&["/getactivescheme"]).output().ok()?;
    parse_scheme(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(windows)]
fn powercfg(args: &[&str]) -> bool {
    command(args).status().is_ok_and(|s| s.success())
}

#[cfg(not(windows))]
fn active_scheme() -> Option<String> {
    None
}

#[cfg(not(windows))]
fn powercfg(_args: &[&str]) -> bool {
    false
}

/// Sends WM_CLOSE to the windows of processes named in `names` (e.g.
/// "chrome.exe"), like clicking their close button, so nothing is lost.
#[cfg(windows)]
fn close_windows_of(names: &[String]) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, GW_OWNER, WM_CLOSE,
    };

    let names: Vec<String> = names.iter().map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()).collect();
    if names.is_empty() {
        return;
    }
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let mut pids: Vec<u32> = sys
        .processes()
        .iter()
        .filter(|(_, p)| names.contains(&p.name().to_string_lossy().to_lowercase()))
        .map(|(pid, _)| pid.as_u32())
        .collect();
    if pids.is_empty() {
        return;
    }

    unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let pids = unsafe { &*(lparam.0 as *const Vec<u32>) };
        let mut pid = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let top_level = GetWindow(hwnd, GW_OWNER).map(|o| o.is_invalid()).unwrap_or(true);
            if pids.contains(&pid) && top_level && IsWindowVisible(hwnd).as_bool() {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut pids as *mut Vec<u32> as isize));
    }
}

#[cfg(not(windows))]
fn close_windows_of(names: &[String]) {
    if !names.is_empty() {
        eprintln!("closing {names:?} ignored outside Windows");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_scheme_guid_in_any_language() {
        let it = "GUID combinazione per il risparmio di energia: 381b4222-f694-41f0-9685-ff5bb260df2e  (Bilanciato)";
        assert_eq!(parse_scheme(it).as_deref(), Some("381b4222-f694-41f0-9685-ff5bb260df2e"));
        assert_eq!(parse_scheme("nessuno"), None);
    }
}
