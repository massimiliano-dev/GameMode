//! Power and volume controls for the quick menu. Outside Windows these are
//! inert so development never shuts down the dev machine.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    Shutdown,
    Restart,
    Sleep,
    /// Restart, and sign in on the Windows desktop once (console mode stays on).
    #[serde(rename = "restartToDesktop")]
    RestartToDesktop,
    Hibernate,
    Lock,
    Logoff,
    /// Restart installing the pending Windows updates on the way.
    #[serde(rename = "updateRestart")]
    UpdateRestart,
    #[serde(rename = "updateShutdown")]
    UpdateShutdown,
}

impl PowerAction {
    /// The Windows session ends: Windows asks GameMode to close first
    /// (WM_QUERYENDSESSION), where the games get closed and saved.
    pub fn ends_session(self) -> bool {
        matches!(
            self,
            PowerAction::Shutdown
                | PowerAction::Restart
                | PowerAction::RestartToDesktop
                | PowerAction::Logoff
                | PowerAction::UpdateRestart
                | PowerAction::UpdateShutdown
        )
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

/// Who changed the volume, carried as the event context of the change so
/// the volume callback (audio_events.rs) can tell GameMode's own changes
/// from everyone else's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The quick menu or the overlay: a panel already showing the value.
    Ui,
    /// Volume keys or the controller combo: needs the on-screen indicator.
    Shortcut,
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
        PowerAction::RestartToDesktop => {
            // Only the flag differs: the shell registration is left alone.
            crate::shell::request_desktop_boot()?;
            exit_windows(EWX_REBOOT | EWX_FORCEIFHUNG, &["/r", "/t", "0"])
        }
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
        PowerAction::UpdateRestart => crate::winupdate::shutdown(true),
        PowerAction::UpdateShutdown => crate::winupdate::shutdown(false),
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
    if matches!(action, PowerAction::RestartToDesktop) {
        crate::shell::request_desktop_boot()?;
    }
    log::warn!("power action {action:?} ignored outside Windows");
    Ok(())
}

#[cfg(not(windows))]
pub fn power_options() -> PowerOptions {
    PowerOptions { sleep: true, hibernate: true }
}

#[cfg(windows)]
mod audio {
    use anyhow::Result;
    use windows::core::GUID;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    use super::{Source, Volume};

    /// Event contexts GameMode passes with its own changes.
    const CONTEXT_UI: GUID = GUID::from_u128(0x3f1c7a52_6b1e_4b7d_9d0a_8c2f4e6a1b01);
    const CONTEXT_SHORTCUT: GUID = GUID::from_u128(0x3f1c7a52_6b1e_4b7d_9d0a_8c2f4e6a1b02);

    fn context(source: Source) -> *const GUID {
        match source {
            Source::Ui => &CONTEXT_UI,
            Source::Shortcut => &CONTEXT_SHORTCUT,
        }
    }

    /// The source behind a notification's event context, if it was ours.
    pub fn source_of(context: &GUID) -> Option<Source> {
        if *context == CONTEXT_UI {
            Some(Source::Ui)
        } else if *context == CONTEXT_SHORTCUT {
            Some(Source::Shortcut)
        } else {
            None
        }
    }

    /// The default output's volume interface (a fresh one each call; the
    /// watcher keeps its own).
    pub fn endpoint() -> Result<IAudioEndpointVolume> {
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

    pub fn set_level(level: f32, source: Source) -> Result<()> {
        unsafe { Ok(endpoint()?.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), context(source))?) }
    }

    pub fn set_muted(muted: bool, source: Source) -> Result<()> {
        unsafe { Ok(endpoint()?.SetMute(muted, context(source))?) }
    }
}

#[cfg(not(windows))]
mod audio {
    use std::sync::Mutex;

    use anyhow::Result;

    use super::{Source, Volume};

    static VOLUME: Mutex<Volume> = Mutex::new(Volume { level: 0.6, muted: false });

    pub fn get() -> Result<Volume> {
        Ok(*VOLUME.lock().unwrap())
    }

    pub fn set_level(level: f32, _source: Source) -> Result<()> {
        VOLUME.lock().unwrap().level = level.clamp(0.0, 1.0);
        Ok(())
    }

    pub fn set_muted(muted: bool, _source: Source) -> Result<()> {
        VOLUME.lock().unwrap().muted = muted;
        Ok(())
    }
}

pub use audio::get as volume;
#[cfg(windows)]
pub use audio::{endpoint, source_of};
pub use audio::{set_level as set_volume_from, set_muted as set_muted_from};

/// Volume changes from GameMode's own panels.
pub fn set_volume(level: f32) -> Result<()> {
    audio::set_level(level, Source::Ui)
}

pub fn set_muted(muted: bool) -> Result<()> {
    audio::set_muted(muted, Source::Ui)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_shutdown_restart_and_logoff_end_the_session() {
        assert!(PowerAction::Shutdown.ends_session());
        assert!(PowerAction::Restart.ends_session());
        assert!(PowerAction::RestartToDesktop.ends_session());
        assert!(PowerAction::Logoff.ends_session());
        assert!(!PowerAction::Sleep.ends_session());
        assert!(!PowerAction::Hibernate.ends_session());
        assert!(!PowerAction::Lock.ends_session());
    }
}
