//! Console mode: GameMode replaces Explorer as the Windows shell for the
//! current user (`HKCU\…\Winlogon\Shell`). Signing in lands straight in
//! GameMode; the desktop is one "Go to desktop" away. Because Explorer isn't
//! there, GameMode also does the shell's chores: starting the user's startup
//! apps, and starting Explorer if GameMode itself goes away (watchdog).

use anyhow::Result;
use serde::Serialize;

/// Passed to the watchdog copy of GameMode: `--watchdog <pid>`.
pub const WATCHDOG_ARG: &str = "--watchdog";
/// Also passed to the watchdog: `--live <live-session file>`, so it can thaw
/// games GameMode left frozen (Quick Resume) if it dies.
pub const LIVE_ARG: &str = "--live";

#[derive(Serialize, Clone, Copy, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ShellStatus {
    /// GameMode is configured as this user's shell.
    pub enabled: bool,
    /// This session started without Explorer (GameMode is the shell now).
    pub active: bool,
    /// This process is the new version started by an update.
    pub after_update: bool,
}

/// Splits a `Run` value into program and arguments (same rules as uninstallers).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn split_command(command: &str) -> (String, String) {
    crate::uninstall::split_command(command)
}

/// Whether a `StartupApproved` value means "disabled in Task Manager".
#[cfg_attr(not(windows), allow(dead_code))]
pub fn approved_disabled(value: &[u8]) -> bool {
    value.first().is_some_and(|b| b & 1 == 1)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::path::Path;

    use anyhow::{Context, Result};
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE};
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowW, GetClassNameW, IsWindowVisible, ShowWindow, SW_HIDE, SW_SHOWNA,
    };
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY};
    use winreg::{RegKey, HKEY};

    use super::{approved_disabled, split_command, WATCHDOG_ARG};

    const WINLOGON: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon";
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    fn exe() -> Result<String> {
        Ok(std::env::current_exe().context("percorso di GameMode")?.to_string_lossy().into_owned())
    }

    /// Explorer's taskbar exists: some shell (Explorer) is running.
    pub fn explorer_running() -> bool {
        unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).is_ok() }
    }

    /// GameMode is this user's shell in the registry.
    pub fn enabled() -> bool {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(WINLOGON)
            .and_then(|k| k.get_value::<String, _>("Shell"))
            .ok()
            .zip(exe().ok())
            .is_some_and(|(value, exe)| value.trim_matches('"').eq_ignore_ascii_case(&exe))
    }

    pub fn set(enabled: bool) -> Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(WINLOGON)?;
        if enabled {
            key.set_value("Shell", &format!("\"{}\"", exe()?))?;
        } else {
            // No per-user value: Windows falls back to Explorer.
            let _ = key.delete_value("Shell");
        }
        Ok(())
    }

    fn spawn_explorer() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let _ = std::process::Command::new(Path::new(&windir).join("explorer.exe")).spawn();
    }

    /// Opens the Windows desktop: starts Explorer (with no shell running it
    /// becomes the full desktop), or shows what console mode hid.
    pub fn start_explorer() {
        super::DESKTOP_WANTED.store(true, std::sync::atomic::Ordering::SeqCst);
        if explorer_running() {
            show_desktop();
        } else {
            spawn_explorer();
        }
    }

    /// Explorer's own windows: the taskbars and the desktop with its icons.
    fn desktop_windows() -> Vec<HWND> {
        unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let found = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
            let mut class = [0u16; 64];
            let len = unsafe { GetClassNameW(hwnd, &mut class) }.max(0) as usize;
            let class = String::from_utf16_lossy(&class[..len]);
            if matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman") {
                found.push(hwnd);
            }
            BOOL(1)
        }
        let mut found: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut found as *mut Vec<HWND> as isize));
        }
        found
    }

    /// Hides the taskbars and desktop icons: Explorer keeps running for the
    /// Xbox app and Game Pass games, but the console stays a console.
    pub fn hide_desktop() {
        for hwnd in desktop_windows() {
            unsafe {
                if IsWindowVisible(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
    }

    pub fn show_desktop() {
        for hwnd in desktop_windows() {
            unsafe {
                if !IsWindowVisible(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_SHOWNA);
                }
            }
        }
    }

    /// Starts a detached copy of GameMode that brings the desktop back if
    /// this process dies, so a crash never leaves a black screen.
    pub fn spawn_watchdog(live_file: &Path) {
        if let Ok(exe) = exe() {
            let _ = std::process::Command::new(exe)
                .args([WATCHDOG_ARG, &std::process::id().to_string(), super::LIVE_ARG])
                .arg(live_file)
                .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
                .spawn();
        }
    }

    /// Body of the watchdog process.
    pub fn run_watchdog(pid: u32) {
        unsafe {
            if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                WaitForSingleObject(handle, INFINITE);
                let _ = CloseHandle(handle);
            }
        }
        // Windows is shutting down or signing out: no desktop to bring back.
        if crate::shutdown::session_ending() {
            return;
        }
        // An update restarts GameMode: give the new version time to come up
        // instead of opening the desktop under the installer.
        if super::update_pending() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
            while std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_secs(2));
                // The new version reads (and clears) the flag itself.
                if other_instance_running() {
                    return;
                }
            }
            super::clear_update_flag();
        }
        // A crash can leave a game frozen by Quick Resume: thaw it.
        if let Some(live_file) = super::watchdog_live_file() {
            crate::live::thaw_frozen_in(&live_file);
        }
        // A clean exit to the desktop already started Explorer.
        std::thread::sleep(std::time::Duration::from_millis(500));
        start_explorer();
    }

    /// Another GameMode (not this watchdog) is running.
    fn other_instance_running() -> bool {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
        let Some(name) = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_os_string())) else {
            return false;
        };
        let me = std::process::id();
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
        sys.processes().iter().any(|(pid, p)| pid.as_u32() != me && p.name().eq_ignore_ascii_case(&name))
    }

    /// Packaged apps (Xbox app, Microsoft Store, Settings) need Explorer:
    /// start it in the background first, in console mode.
    pub fn ensure_explorer() {
        if !explorer_running() {
            spawn_explorer();
            // Hide the taskbar as soon as it shows up, while Explorer settles.
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if !super::DESKTOP_WANTED.load(std::sync::atomic::Ordering::SeqCst) {
                    hide_desktop();
                }
            }
        }
    }

    fn run_values(root: HKEY, view: u32) -> Vec<(String, String)> {
        let Ok(key) = RegKey::predef(root).open_subkey_with_flags(RUN, KEY_READ | view) else { return Vec::new() };
        key.enum_values()
            .flatten()
            .filter_map(|(name, _)| Some((name.clone(), key.get_value::<String, _>(&name).ok()?)))
            .collect()
    }

    fn disabled_in_task_manager(root: HKEY, name: &str) -> bool {
        RegKey::predef(root)
            .open_subkey(APPROVED)
            .and_then(|k| k.get_raw_value(name))
            .is_ok_and(|v| approved_disabled(&v.bytes))
    }

    /// Starts what Explorer would have at sign-in: the `Run` entries (minus
    /// those switched off in Task Manager and GameMode itself) and the
    /// Startup folders.
    pub fn run_startup_apps() {
        use windows::core::HSTRING;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

        let me = exe().unwrap_or_default().to_lowercase();
        let mut entries: Vec<(HKEY, String, String)> = Vec::new();
        for (name, cmd) in run_values(HKEY_CURRENT_USER, 0) {
            entries.push((HKEY_CURRENT_USER, name, cmd));
        }
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            for (name, cmd) in run_values(HKEY_LOCAL_MACHINE, view) {
                if !entries.iter().any(|(_, n, _)| *n == name) {
                    entries.push((HKEY_LOCAL_MACHINE, name, cmd));
                }
            }
        }
        for (root, name, cmd) in entries {
            if cmd.to_lowercase().contains(&me) || disabled_in_task_manager(root, &name) {
                continue;
            }
            let expanded = expand_env(&cmd);
            let (program, args) = split_command(&expanded);
            if let Err(err) = std::process::Command::new(&program).raw_arg(&args).spawn() {
                eprintln!("startup app {name}: {err}");
            }
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        }
        let folders = [("APPDATA", r"Microsoft\Windows\Start Menu\Programs\Startup"), ("PROGRAMDATA", r"Microsoft\Windows\Start Menu\Programs\StartUp")];
        for (var, sub) in folders {
            let Some(base) = std::env::var_os(var) else { continue };
            for entry in std::fs::read_dir(Path::new(&base).join(sub)).into_iter().flatten().flatten() {
                let path = entry.path();
                // A folder would open in Explorer, which then takes over as the shell.
                if path.is_dir() || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ini")) {
                    continue;
                }
                // ShellExecute resolves shortcuts in-process, without Explorer.
                let file = HSTRING::from(path.as_os_str());
                unsafe {
                    ShellExecuteW(None, PCWSTR::null(), &file, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
                }
            }
        }
    }

    /// Expands `%VAR%` references, as REG_EXPAND_SZ values contain.
    fn expand_env(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(start) = rest.find('%') {
            let Some(len) = rest[start + 1..].find('%') else { break };
            let name = &rest[start + 1..start + 1 + len];
            out.push_str(&rest[..start]);
            match std::env::var(name) {
                Ok(value) if !name.is_empty() => out.push_str(&value),
                _ => out.push_str(&rest[start..start + len + 2]),
            }
            rest = &rest[start + len + 2..];
        }
        out.push_str(rest);
        out
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;

    static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    pub fn enabled() -> bool {
        ENABLED.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn explorer_running() -> bool {
        true
    }

    pub fn set(enabled: bool) -> Result<()> {
        ENABLED.store(enabled, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    pub fn start_explorer() {}

    pub fn ensure_explorer() {}

    pub fn hide_desktop() {}

    pub fn spawn_watchdog(_live_file: &std::path::Path) {}

    pub fn run_watchdog(_pid: u32) {}

    pub fn run_startup_apps() {}
}

pub use imp::{ensure_explorer, run_startup_apps, run_watchdog, set, spawn_watchdog, start_explorer};

/// The user asked for the Windows desktop ("Apri il desktop di Windows").
static DESKTOP_WANTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// In console mode Explorer may run in the background (the Xbox app and
/// Game Pass games need it), but its taskbar and desktop stay hidden until
/// the user asks for the desktop. Explorer brings the taskbar back now and
/// then, so this keeps checking.
///
/// With the desktop hidden, nothing in front means a black screen (a game
/// closed and GameMode didn't make it back): then GameMode comes back.
pub fn keep_desktop_hidden(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut empty_for = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if DESKTOP_WANTED.load(std::sync::atomic::Ordering::SeqCst) || !status().active {
                empty_for = 0;
                continue;
            }
            if imp::explorer_running() {
                imp::hide_desktop();
            }
            empty_for = if crate::window::nothing_in_front() { empty_for + 1 } else { 0 };
            if empty_for >= BLACK_SCREEN_SECS {
                empty_for = 0;
                crate::window::bring_to_front(&app);
            }
        }
    });
}

/// Seconds of an empty screen before GameMode comes back by itself.
const BLACK_SCREEN_SECS: u32 = 3;

/// GameMode is back in front: in console mode, the desktop goes away again.
pub fn console_in_front() {
    if status().active {
        DESKTOP_WANTED.store(false, std::sync::atomic::Ordering::SeqCst);
        imp::hide_desktop();
    }
}

/// How this GameMode process came up, fixed at the first check (during
/// setup): Explorer started later in the background for the Xbox app or
/// the Store doesn't turn console mode off.
struct Start {
    as_shell: bool,
    after_update: bool,
}

static START: std::sync::OnceLock<Start> = std::sync::OnceLock::new();

fn start() -> &'static Start {
    START.get_or_init(|| {
        let update = take_update_flag();
        Start { as_shell: !imp::explorer_running() || update == Some(true), after_update: update.is_some() }
    })
}

pub fn status() -> ShellStatus {
    let enabled = imp::enabled();
    ShellStatus { enabled, active: enabled && start().as_shell, after_update: start().after_update }
}

/// Console mode with nothing underneath GameMode: no desktop to step aside to.
pub fn no_desktop() -> bool {
    status().active && !imp::explorer_running()
}

/// This process is the new version started by an update.
pub fn restarted_for_update() -> bool {
    start().after_update
}

fn update_flag() -> std::path::PathBuf {
    std::env::temp_dir().join("gamemode-updating")
}

/// Marks that GameMode is about to restart for an update (read by the
/// watchdog and by the new version), noting whether it was in console mode.
pub fn mark_updating() {
    let _ = std::fs::write(update_flag(), format!("{} {}", crate::model::now(), u8::from(status().active)));
}

/// `(written at, was in console mode)` of a recent update flag.
fn read_update_flag() -> Option<(i64, bool)> {
    let text = std::fs::read_to_string(update_flag()).ok()?;
    let mut parts = text.split_whitespace();
    let at = parts.next()?.parse::<i64>().ok()?;
    let console = parts.next() == Some("1");
    (crate::model::now() - at < 300).then_some((at, console))
}

#[cfg_attr(not(windows), allow(dead_code))]
fn update_pending() -> bool {
    read_update_flag().is_some()
}

/// Consumes the update flag: `Some(was in console mode)` right after an update.
fn take_update_flag() -> Option<bool> {
    let flag = read_update_flag().map(|(_, console)| console);
    clear_update_flag();
    flag
}

#[cfg_attr(not(windows), allow(dead_code))]
fn clear_update_flag() {
    let _ = std::fs::remove_file(update_flag());
}

/// Links handled by packaged apps, which don't start without Explorer.
pub fn needs_explorer(uri: &str) -> bool {
    ["msxbox:", "ms-windows-store:", "ms-settings:", "ms-xbl-", "xbox:"].iter().any(|p| uri.starts_with(p))
}

/// Before opening `uri`: in console mode, packaged apps get Explorer first.
pub fn prepare_for(uri: &str) {
    if needs_explorer(uri) && status().active {
        ensure_explorer();
    }
}

/// `--watchdog <pid>` on the command line.
pub fn watchdog_pid() -> Option<u32> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == WATCHDOG_ARG)?;
    args.get(i + 1)?.parse().ok()
}

/// `--live <file>` on the watchdog's command line.
#[cfg_attr(not(windows), allow(dead_code))]
fn watchdog_live_file() -> Option<std::path::PathBuf> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let i = args.iter().position(|a| a == LIVE_ARG)?;
    args.get(i + 1).map(std::path::PathBuf::from)
}

pub fn set_enabled(enabled: bool) -> Result<ShellStatus> {
    set(enabled)?;
    Ok(status())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_task_manager_switches() {
        assert!(!approved_disabled(&[2, 0, 0, 0]));
        assert!(approved_disabled(&[3, 0, 0, 0]));
        assert!(!approved_disabled(&[]));
    }
}
