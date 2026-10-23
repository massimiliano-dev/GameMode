//! Power and volume controls for the quick menu. Outside Windows these are
//! inert so development never shuts down the dev machine.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    Shutdown,
    Restart,
    Sleep,
    Hibernate,
    Lock,
    Logoff,
}

impl PowerAction {
    /// The Windows session ends: Windows asks GameMode to close first
    /// (WM_QUERYENDSESSION), where the games get closed and saved.
    pub fn ends_session(self) -> bool {
        matches!(self, PowerAction::Shutdown | PowerAction::Restart | PowerAction::Logoff)
    }
}

/// Which power actions this PC supports: sleep is missing on some desktops,
/// hibernation is often switched off (`powercfg /h off`) on gaming PCs.
#[derive(Serialize, Clone, Copy, Debug)]
pub struct PowerOptions {
    pub sleep: bool,
    pub hibernate: bool,
}

#[derive(Serialize, Clone, Copy, Debug)]
pub struct Volume {
    /// 0.0 – 1.0
    pub level: f32,
    pub muted: bool,
}

#[cfg(windows)]
pub fn power(action: PowerAction) -> Result<()> {
    use windows::Win32::System::Power::{IsPwrHibernateAllowed, SetSuspendState};
    use windows::Win32::System::Shutdown::{
        LockWorkStation, EWX_FORCEIFHUNG, EWX_LOGOFF, EWX_POWEROFF, EWX_REBOOT, EWX_SHUTDOWN,
    };
    match action {
        PowerAction::Shutdown => exit_windows(EWX_SHUTDOWN | EWX_POWEROFF | EWX_FORCEIFHUNG, &["/s", "/t", "0"]),
        PowerAction::Restart => exit_windows(EWX_REBOOT | EWX_FORCEIFHUNG, &["/r", "/t", "0"]),
        PowerAction::Logoff => exit_windows(EWX_LOGOFF | EWX_FORCEIFHUNG, &["/l"]),
        PowerAction::Sleep => unsafe {
            if !SetSuspendState(false, false, false) {
                anyhow::bail!("sospensione non riuscita");
            }
            Ok(())
        },
        PowerAction::Hibernate => unsafe {
            if !IsPwrHibernateAllowed() {
                anyhow::bail!("L'ibernazione è disattivata su questo PC");
            }
            if !SetSuspendState(true, false, false) {
                anyhow::bail!("ibernazione non riuscita");
            }
            Ok(())
        },
        PowerAction::Lock => unsafe { Ok(LockWorkStation()?) },
    }
}

/// Ends the session through ExitWindowsEx, so Windows asks every app
/// (GameMode included) to close first; `shutdown.exe` is the fallback.
/// EWX_FORCEIFHUNG only pushes apps that stopped answering, never EWX_FORCE.
#[cfg(windows)]
fn exit_windows(flags: windows::Win32::System::Shutdown::EXIT_WINDOWS_FLAGS, fallback: &[&str]) -> Result<()> {
    use windows::Win32::System::Shutdown::{ExitWindowsEx, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_APPLICATION};
    if let Err(err) = enable_shutdown_privilege() {
        eprintln!("shutdown privilege: {err}");
    }
    let result = unsafe { ExitWindowsEx(flags, SHTDN_REASON_MAJOR_APPLICATION | SHTDN_REASON_FLAG_PLANNED) };
    if let Err(err) = result {
        eprintln!("ExitWindowsEx: {err}; falling back to shutdown.exe");
        std::process::Command::new("shutdown").args(fallback).spawn()?;
    }
    Ok(())
}

/// Every interactive user holds SeShutdownPrivilege, disabled: enabling it
/// needs no elevation (shutdown.exe does the same).
#[cfg(windows)]
fn enable_shutdown_privilege() -> Result<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED, SE_SHUTDOWN_NAME,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)?;
        let mut luid = LUID::default();
        let result = LookupPrivilegeValueW(PCWSTR::null(), SE_SHUTDOWN_NAME, &mut luid).and_then(|_| {
            let privileges = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }],
            };
            AdjustTokenPrivileges(token, false, Some(&privileges), 0, None, None)
        });
        let _ = CloseHandle(token);
        Ok(result?)
    }
}

#[cfg(windows)]
pub fn power_options() -> PowerOptions {
    use windows::Win32::System::Power::{IsPwrHibernateAllowed, IsPwrSuspendAllowed};
    unsafe { PowerOptions { sleep: IsPwrSuspendAllowed(), hibernate: IsPwrHibernateAllowed() } }
}

#[cfg(not(windows))]
pub fn power(action: PowerAction) -> Result<()> {
    eprintln!("power action {action:?} ignored outside Windows");
    Ok(())
}

#[cfg(not(windows))]
pub fn power_options() -> PowerOptions {
    PowerOptions { sleep: true, hibernate: true }
}

#[cfg(windows)]
mod audio {
    use anyhow::Result;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    use super::Volume;

    fn endpoint() -> Result<IAudioEndpointVolume> {
        unsafe {
            // S_FALSE / RPC_E_CHANGED_MODE just mean COM is already set up.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            Ok(device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)?)
        }
    }

    pub fn get() -> Result<Volume> {
        let ep = endpoint()?;
        unsafe { Ok(Volume { level: ep.GetMasterVolumeLevelScalar()?, muted: ep.GetMute()?.as_bool() }) }
    }

    pub fn set_level(level: f32) -> Result<()> {
        unsafe { Ok(endpoint()?.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null())?) }
    }

    pub fn set_muted(muted: bool) -> Result<()> {
        unsafe { Ok(endpoint()?.SetMute(muted, std::ptr::null())?) }
    }
}

#[cfg(not(windows))]
mod audio {
    use std::sync::Mutex;

    use anyhow::Result;

    use super::Volume;

    static VOLUME: Mutex<Volume> = Mutex::new(Volume { level: 0.6, muted: false });

    pub fn get() -> Result<Volume> {
        Ok(*VOLUME.lock().unwrap())
    }

    pub fn set_level(level: f32) -> Result<()> {
        VOLUME.lock().unwrap().level = level.clamp(0.0, 1.0);
        Ok(())
    }

    pub fn set_muted(muted: bool) -> Result<()> {
        VOLUME.lock().unwrap().muted = muted;
        Ok(())
    }
}

pub use audio::{get as volume, set_level as set_volume, set_muted};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_shutdown_restart_and_logoff_end_the_session() {
        assert!(PowerAction::Shutdown.ends_session());
        assert!(PowerAction::Restart.ends_session());
        assert!(PowerAction::Logoff.ends_session());
        assert!(!PowerAction::Sleep.ends_session());
        assert!(!PowerAction::Hibernate.ends_session());
        assert!(!PowerAction::Lock.ends_session());
    }
}
