//! Start with Windows. On Windows this writes the `Run` key itself: the
//! autostart plugin stores the executable path unquoted, which Windows can't
//! start when the path contains a space (e.g. `C:\Users\Nome Cognome\…`).
//! Elsewhere the plugin is fine.

use anyhow::Result;
use tauri::AppHandle;

/// Passed on the command line when Windows starts GameMode at login.
pub const ARG: &str = "--autostart";

pub fn launched_at_login() -> bool {
    std::env::args().any(|a| a == ARG)
}

#[cfg(windows)]
mod imp {
    use std::borrow::Cow;

    use anyhow::{Context, Result};
    use tauri::AppHandle;
    use winreg::enums::{HKEY_CURRENT_USER, REG_BINARY};
    use winreg::{RegKey, RegValue};

    use super::ARG;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    /// Where "Startup apps" (Settings / Task Manager) records the on/off switch.
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    /// The Run value is named after the product, so GameMode and GameMode
    /// Beta keep separate entries.
    fn name(app: &AppHandle) -> String {
        app.package_info().name.clone()
    }
    /// First byte 0x02 = enabled; odd values mean the user switched it off.
    const ENABLED: [u8; 12] = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

    fn command() -> Result<String> {
        let exe = std::env::current_exe().context("percorso di GameMode")?;
        Ok(format!("\"{}\" {ARG}", exe.display()))
    }

    fn hkcu() -> RegKey {
        RegKey::predef(HKEY_CURRENT_USER)
    }

    pub fn is_enabled(app: &AppHandle) -> Result<bool> {
        let Ok(value) = hkcu().open_subkey(RUN).and_then(|k| k.get_value::<String, _>(name(app))) else {
            return Ok(false);
        };
        let approved = hkcu()
            .open_subkey(APPROVED)
            .and_then(|k| k.get_raw_value(name(app)))
            .map(|v| v.bytes.first().is_none_or(|b| b & 1 == 0))
            .unwrap_or(true);
        Ok(value == command()? && approved)
    }

    pub fn set(app: &AppHandle, enabled: bool) -> Result<()> {
        let (run, _) = hkcu().create_subkey(RUN)?;
        if enabled {
            run.set_value(name(app), &command()?)?;
            let (approved, _) = hkcu().create_subkey(APPROVED)?;
            approved.set_raw_value(name(app), &RegValue { bytes: Cow::Borrowed(&ENABLED), vtype: REG_BINARY })?;
        } else {
            let _ = run.delete_value(name(app));
            if let Ok(approved) = hkcu().open_subkey_with_flags(APPROVED, winreg::enums::KEY_WRITE) {
                let _ = approved.delete_value(name(app));
            }
        }
        Ok(())
    }

    /// Rewrites an entry left by older versions (unquoted path, or pointing at
    /// a previous install location) so it actually starts this executable.
    pub fn repair(app: &AppHandle) {
        let Ok(value) = hkcu().open_subkey(RUN).and_then(|k| k.get_value::<String, _>(name(app))) else { return };
        if let Ok(expected) = command() {
            if value != expected {
                if let Ok((run, _)) = hkcu().create_subkey(RUN) {
                    let _ = run.set_value(name(app), &expected);
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use anyhow::Result;
    use tauri::AppHandle;
    use tauri_plugin_autostart::ManagerExt;

    pub fn is_enabled(app: &AppHandle) -> Result<bool> {
        Ok(app.autolaunch().is_enabled()?)
    }

    pub fn set(app: &AppHandle, enabled: bool) -> Result<()> {
        let manager = app.autolaunch();
        if enabled { manager.enable()? } else { manager.disable()? }
        Ok(())
    }

    pub fn repair(_app: &AppHandle) {}
}

pub fn is_enabled(app: &AppHandle) -> Result<bool> {
    imp::is_enabled(app)
}

pub fn set(app: &AppHandle, enabled: bool) -> Result<()> {
    imp::set(app, enabled)
}

pub fn repair(app: &AppHandle) {
    imp::repair(app)
}
