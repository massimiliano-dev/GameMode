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
    /// Console mode is on, but this session was signed in on the Windows
    /// desktop for once ("Riavvia in modalità desktop" or B+View held at
    /// start): GameMode waits in the tray, and the next restart is back in
    /// console mode.
    pub desktop_once: bool,
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

    /// The session is ending (a restart in progress, maybe waiting on apps
    /// that block it).
    pub fn shutting_down() -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};
        unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
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

    /// Starts explorer.exe: with no shell running, it becomes the desktop.
    pub fn spawn_explorer() {
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

    pub fn shutting_down() -> bool {
        false
    }

    pub fn set(enabled: bool) -> Result<()> {
        ENABLED.store(enabled, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    pub fn start_explorer() {}

    pub fn spawn_explorer() {}

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
    desktop_once: bool,
}

static START: std::sync::OnceLock<Start> = std::sync::OnceLock::new();

fn start() -> &'static Start {
    START.get_or_init(|| {
        let update = take_update_flag();
        let explorer = imp::explorer_running();
        // A stale flag is consumed even when it no longer matters.
        let flag = take_boot_flag();
        // Signed in as the shell, but the user asked for the desktop this
        // once: Explorer gets the session, GameMode steps aside.
        let asked = !explorer && update.is_none() && (flag || crate::gamepad::desktop_combo_held());
        if asked {
            DESKTOP_WANTED.store(true, std::sync::atomic::Ordering::SeqCst);
            imp::spawn_explorer();
        }
        // Updated during a "desktop once" session: still on that desktop,
        // Explorer already the shell.
        let desktop_once = asked || update.is_some_and(|u| u.desktop_once);
        let console = update.is_some_and(|u| u.console);
        Start { as_shell: (!explorer || console) && !desktop_once, after_update: update.is_some(), desktop_once }
    })
}

pub fn status() -> ShellStatus {
    let enabled = imp::enabled();
    let start = start();
    ShellStatus {
        enabled,
        active: enabled && start.as_shell,
        after_update: start.after_update,
        // Console mode turned off in this session: the next sign-in is the
        // desktop anyway, nothing to "go back" to.
        desktop_once: enabled && start.desktop_once,
    }
}

/// Seconds given to Explorer to put up its taskbar in a "desktop once" sign-in.
const DESKTOP_ONCE_WAIT_SECS: u32 = 20;

/// "Desktop once": GameMode hid itself trusting Explorer to become the
/// shell. If no taskbar shows up (explorer.exe didn't start, or crashed),
/// the session has no shell and no window: GameMode comes back instead.
pub fn guard_desktop_once(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        for _ in 0..DESKTOP_ONCE_WAIT_SECS {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if imp::explorer_running() {
                return;
            }
        }
        eprintln!("desktop once: Explorer never showed up, back to GameMode");
        crate::window::bring_to_front(&app);
    });
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
/// watchdog and by the new version), noting whether it was in console mode
/// or on the desktop for once.
pub fn mark_updating() {
    let status = status();
    let text = format!("{} {} {}", crate::model::now(), u8::from(status.active), u8::from(status.desktop_once));
    let _ = std::fs::write(update_flag(), text);
}

/// How the version being replaced was running.
#[derive(Clone, Copy, Debug, PartialEq)]
struct UpdateFlag {
    console: bool,
    desktop_once: bool,
}

/// Parses the update flag's text (`<at> <console> [<desktop once>]`, the
/// last field missing when written by older versions) if still recent.
fn parse_update_flag(text: &str, now: i64) -> Option<UpdateFlag> {
    let mut parts = text.split_whitespace();
    let at = parts.next()?.parse::<i64>().ok()?;
    let console = parts.next() == Some("1");
    let desktop_once = parts.next() == Some("1");
    (now - at < 300).then_some(UpdateFlag { console, desktop_once })
}

fn read_update_flag() -> Option<UpdateFlag> {
    parse_update_flag(&std::fs::read_to_string(update_flag()).ok()?, crate::model::now())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn update_pending() -> bool {
    read_update_flag().is_some()
}

/// Consumes the update flag: `Some` right after an update.
fn take_update_flag() -> Option<UpdateFlag> {
    let flag = read_update_flag();
    clear_update_flag();
    flag
}

#[cfg_attr(not(windows), allow(dead_code))]
fn clear_update_flag() {
    let _ = std::fs::remove_file(update_flag());
}

fn boot_flag() -> std::path::PathBuf {
    std::env::temp_dir().join("gamemode-desktop-once")
}

/// A boot flag older than this is a restart that never happened.
const BOOT_FLAG_MAX_AGE: i64 = 24 * 3600;

/// `shutdown /r` doesn't force apps to close: one refusing to end the
/// session cancels the restart. If this process is still alive after this
/// long and the session isn't ending, the restart didn't happen and the
/// flag must not outlive it.
const BOOT_FLAG_GRACE: std::time::Duration = std::time::Duration::from_secs(90);

/// Longest wait on a session that is still ending (Windows asking what to
/// do with apps that block the restart) before the flag is withdrawn.
const BOOT_FLAG_SHUTDOWN_WAIT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// Asks for the next sign-in to land on the Windows desktop instead of
/// GameMode, once, without touching the shell registration. Written right
/// before "Riavvia in modalità desktop" restarts the PC; taken back if the
/// PC is still up (restart cancelled) after `BOOT_FLAG_GRACE`.
pub fn request_desktop_boot() -> Result<()> {
    let token = boot_flag_token(crate::model::now(), std::process::id());
    std::fs::write(boot_flag(), &token)?;
    std::thread::spawn(move || {
        std::thread::sleep(BOOT_FLAG_GRACE);
        let since = std::time::Instant::now();
        while imp::shutting_down() && since.elapsed() < BOOT_FLAG_SHUTDOWN_WAIT {
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        // A later request wrote its own flag: that one is not ours to take.
        if std::fs::read_to_string(boot_flag()).is_ok_and(|t| t == token) && std::fs::remove_file(boot_flag()).is_ok() {
            eprintln!("restart to desktop didn't happen: flag withdrawn");
        }
    });
    Ok(())
}

/// Unique text of one request: the time it was made (what
/// `boot_flag_valid` reads) and a nonce telling requests apart.
fn boot_flag_token(now: i64, pid: u32) -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("{now} {pid}-{}-{nanos}", SEQ.fetch_add(1, Ordering::SeqCst))
}

/// Whether the flag's text (written by `request_desktop_boot`) still counts:
/// written before this boot (so a restart really followed it, and a
/// watchdog restart of GameMode in the same session doesn't see it) and
/// not too long ago.
fn boot_flag_valid(text: &str, boot_time: i64, now: i64) -> bool {
    let at = text.split_whitespace().next().and_then(|t| t.parse::<i64>().ok());
    at.is_some_and(|at| at < boot_time && now - at < BOOT_FLAG_MAX_AGE)
}

/// Consumes the "desktop once" flag: true when a recent one was there.
fn take_boot_flag() -> bool {
    let path = boot_flag();
    let boot_time = sysinfo::System::boot_time() as i64;
    let valid = std::fs::read_to_string(&path).is_ok_and(|t| boot_flag_valid(&t, boot_time, crate::model::now()));
    let _ = std::fs::remove_file(path);
    valid
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

    #[test]
    fn boot_flag_expires_and_rejects_garbage() {
        assert!(boot_flag_valid("1000\n", 1030, 1000 + 60));
        assert!(boot_flag_valid("1000", 1030, 1000 + BOOT_FLAG_MAX_AGE - 1));
        assert!(!boot_flag_valid("1000", 1030, 1000 + BOOT_FLAG_MAX_AGE));
        assert!(!boot_flag_valid("", 1030, 1000));
        assert!(!boot_flag_valid("soon", 1030, 1000));
    }

    #[test]
    fn boot_flag_token_is_unique_and_readable() {
        let a = boot_flag_token(1000, 7);
        let b = boot_flag_token(1000, 7);
        assert_ne!(a, b);
        assert!(boot_flag_valid(&a, 1030, 1000 + 60));
    }

    #[test]
    fn update_flag_remembers_desktop_once() {
        let flag = |console, desktop_once| Some(UpdateFlag { console, desktop_once });
        assert_eq!(parse_update_flag("1000 1", 1010), flag(true, false));
        assert_eq!(parse_update_flag("1000 0 1", 1010), flag(false, true));
        assert_eq!(parse_update_flag("1000 0 0\n", 1010), flag(false, false));
        assert_eq!(parse_update_flag("1000 0 1", 1000 + 300), None);
        assert_eq!(parse_update_flag("", 1010), None);
    }

    #[test]
    fn boot_flag_needs_a_boot_after_it() {
        // Written in this very session (restart cancelled, GameMode restarted).
        assert!(!boot_flag_valid("1000", 900, 1000 + 60));
        assert!(!boot_flag_valid("1000", 1000, 1000 + 60));
    }
}
