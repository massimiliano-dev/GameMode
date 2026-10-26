//! Performance mode while a game runs: the "High performance" power plan and
//! politely closing apps the user listed (browsers, chat apps…). Everything
//! is undone when the game ends. The previous power plan is remembered in
//! the live-session file, so it comes back even after a crash.
//!
//! Power plans go through powrprof (PowerGetActiveScheme and friends), no
//! `powercfg` process. The live-session file keeps the GUID as text, the
//! same format `powercfg` printed, so older files still restore.

/// "High performance" (`GUID_MIN_POWER_SAVINGS`).
const HIGH_PERFORMANCE: u128 = 0x8c5e7fda_e8bf_4a96_9a85_a6e23a8c635c;
/// "Ultimate performance" ("Prestazioni ottimali"), when the user added it.
const ULTIMATE_PERFORMANCE: u128 = 0xe9a42b02_d5df_448d_aa00_03f14749eb61;

pub fn begin(close_apps: &[String]) {
    if let Some(current) = imp::active_scheme() {
        let schemes = imp::schemes();
        let personality = schemes.iter().find(|(g, _)| *g == current).and_then(|(_, p)| *p);
        if !is_fast(current, personality) {
            match pick_scheme(&schemes) {
                Some(fast) if imp::set_active(fast) => crate::live::set_power_scheme(Some(format_guid(current))),
                Some(fast) => log::warn!("performance mode: can't switch to power plan {}", format_guid(fast)),
                None => log::info!("performance mode: no high-performance power plan on this PC"),
            }
        }
    }
    close_windows_of(close_apps);
}

/// Restores the previous power plan, if one was changed (also by a crashed
/// GameMode, whose plan survives in the live-session file).
pub fn end() {
    if let Some(previous) = crate::live::take_power_scheme() {
        match parse_guid(&previous) {
            Some(guid) if imp::set_active(guid) => {}
            _ => log::warn!("performance mode: can't restore power plan {previous}"),
        }
    }
}

/// Already a plan made for speed: nothing to switch.
fn is_fast(scheme: u128, personality: Option<u128>) -> bool {
    scheme == HIGH_PERFORMANCE || scheme == ULTIMATE_PERFORMANCE || personality == Some(HIGH_PERFORMANCE)
}

/// The plan to play with among `(scheme, personality)`: "High performance",
/// else "Ultimate performance", else any plan built on high performance
/// (OEM plans). Laptops with Modern Standby often have Balanced only.
fn pick_scheme(schemes: &[(u128, Option<u128>)]) -> Option<u128> {
    let has = |guid| schemes.iter().any(|(g, _)| *g == guid);
    [HIGH_PERFORMANCE, ULTIMATE_PERFORMANCE]
        .into_iter()
        .find(|g| has(*g))
        .or_else(|| schemes.iter().find(|(_, p)| *p == Some(HIGH_PERFORMANCE)).map(|(g, _)| *g))
}

/// `8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c`, as `powercfg` writes it.
fn format_guid(guid: u128) -> String {
    let hex = format!("{guid:032x}");
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// Reads `format_guid`'s text back (braces and upper case tolerated).
fn parse_guid(text: &str) -> Option<u128> {
    let text = text.trim().trim_start_matches('{').trim_end_matches('}');
    let groups: Vec<&str> = text.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|g| g.len()).collect();
    if lengths != [8, 4, 4, 4, 12] || !groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_hexdigit())) {
        return None;
    }
    u128::from_str_radix(&groups.concat(), 16).ok()
}

#[cfg(windows)]
mod imp {
    use windows::core::GUID;
    use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL};
    use windows::Win32::System::Power::{
        PowerEnumerate, PowerGetActiveScheme, PowerReadACValue, PowerSetActiveScheme, ACCESS_SCHEME,
    };
    use windows::Win32::System::SystemServices::{GUID_POWERSCHEME_PERSONALITY, NO_SUBGROUP_GUID};

    pub fn active_scheme() -> Option<u128> {
        let mut active: *mut GUID = std::ptr::null_mut();
        unsafe {
            if PowerGetActiveScheme(None, &mut active) != ERROR_SUCCESS || active.is_null() {
                return None;
            }
            let guid = (*active).to_u128();
            // Allocated by powrprof with LocalAlloc.
            let _ = LocalFree(Some(HLOCAL(active.cast())));
            Some(guid)
        }
    }

    pub fn set_active(scheme: u128) -> bool {
        let guid = GUID::from_u128(scheme);
        unsafe { PowerSetActiveScheme(None, Some(&guid)) == ERROR_SUCCESS }
    }

    /// Every power plan on this PC, with its personality (the stock plan it
    /// was built from) when readable.
    pub fn schemes() -> Vec<(u128, Option<u128>)> {
        let mut found = Vec::new();
        for index in 0.. {
            let mut guid = GUID::zeroed();
            let mut size = std::mem::size_of::<GUID>() as u32;
            let buffer = (&mut guid as *mut GUID).cast::<u8>();
            let code = unsafe { PowerEnumerate(None, None, None, ACCESS_SCHEME, index, Some(buffer), &mut size) };
            if code != ERROR_SUCCESS {
                // ERROR_NO_MORE_ITEMS, or trouble: either way the list ends.
                break;
            }
            found.push((guid.to_u128(), personality(&guid)));
        }
        found
    }

    fn personality(scheme: &GUID) -> Option<u128> {
        let mut value = GUID::zeroed();
        let mut size = std::mem::size_of::<GUID>() as u32;
        let code = unsafe {
            PowerReadACValue(
                None,
                Some(scheme),
                Some(&NO_SUBGROUP_GUID),
                Some(&GUID_POWERSCHEME_PERSONALITY),
                None,
                Some((&mut value as *mut GUID).cast::<u8>()),
                Some(&mut size),
            )
        };
        (code == ERROR_SUCCESS && size as usize == std::mem::size_of::<GUID>()).then(|| value.to_u128())
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn active_scheme() -> Option<u128> {
        None
    }

    pub fn set_active(_scheme: u128) -> bool {
        false
    }

    pub fn schemes() -> Vec<(u128, Option<u128>)> {
        Vec::new()
    }
}

/// Sends WM_CLOSE to the windows of processes named in `names` (e.g.
/// "chrome.exe"), like clicking their close button, so nothing is lost.
#[cfg(windows)]
fn close_windows_of(names: &[String]) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

    let names: Vec<String> = names.iter().map(|n| n.trim().to_lowercase()).filter(|n| !n.is_empty()).collect();
    if names.is_empty() {
        return;
    }
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    let pids: Vec<u32> = sys
        .processes()
        .iter()
        .filter(|(_, p)| names.contains(&p.name().to_string_lossy().to_lowercase()))
        .map(|(pid, _)| pid.as_u32())
        .collect();
    close_windows(&pids);
}

/// Sends WM_CLOSE to the visible top-level windows of `pids`, like clicking
/// their close button: the app (or game) gets to save and quit on its own.
#[cfg(windows)]
pub fn close_windows(pids: &[u32]) {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, GW_OWNER, WM_CLOSE,
    };

    let mut pids: Vec<u32> = pids.to_vec();
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
        log::warn!("closing {names:?} ignored outside Windows");
    }
}

#[cfg(not(windows))]
pub fn close_windows(pids: &[u32]) {
    if !pids.is_empty() {
        eprintln!("closing windows of {pids:?} ignored outside Windows");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BALANCED: u128 = 0x381b4222_f694_41f0_9685_ff5bb260df2e;
    const OEM_TURBO: u128 = 0x11111111_2222_3333_4444_555555555555;

    #[test]
    fn guids_keep_powercfg_format() {
        let text = "381b4222-f694-41f0-9685-ff5bb260df2e";
        assert_eq!(format_guid(BALANCED), text);
        assert_eq!(parse_guid(text), Some(BALANCED));
        assert_eq!(parse_guid("{381B4222-F694-41F0-9685-FF5BB260DF2E}"), Some(BALANCED));
        assert_eq!(format_guid(HIGH_PERFORMANCE), "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c");
        assert_eq!(parse_guid("nessuno"), None);
        assert_eq!(parse_guid("381b4222f69441f09685ff5bb260df2e"), None);
        assert_eq!(parse_guid("381b4222-f694-41f0-9685-ff5bb260df2g"), None);
    }

    #[test]
    fn prefers_high_performance_then_ultimate_then_oem() {
        let balanced = (BALANCED, Some(BALANCED));
        let high = (HIGH_PERFORMANCE, Some(HIGH_PERFORMANCE));
        let ultimate = (ULTIMATE_PERFORMANCE, Some(HIGH_PERFORMANCE));
        let oem = (OEM_TURBO, Some(HIGH_PERFORMANCE));
        assert_eq!(pick_scheme(&[balanced, ultimate, high]), Some(HIGH_PERFORMANCE));
        assert_eq!(pick_scheme(&[balanced, oem, ultimate]), Some(ULTIMATE_PERFORMANCE));
        assert_eq!(pick_scheme(&[balanced, oem]), Some(OEM_TURBO));
        assert_eq!(pick_scheme(&[balanced]), None);
        assert_eq!(pick_scheme(&[]), None);
    }

    #[test]
    fn fast_plans_are_left_alone() {
        assert!(is_fast(HIGH_PERFORMANCE, None));
        assert!(is_fast(ULTIMATE_PERFORMANCE, None));
        assert!(is_fast(OEM_TURBO, Some(HIGH_PERFORMANCE)));
        assert!(!is_fast(BALANCED, Some(BALANCED)));
        assert!(!is_fast(OEM_TURBO, None));
    }
}
