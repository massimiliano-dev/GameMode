//! Power and volume controls for the quick menu. Outside Windows these are
//! inert so development never shuts down the dev machine.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub enum PowerAction {
    Shutdown,
    Restart,
    Sleep,
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
    use std::process::Command;
    match action {
        PowerAction::Shutdown => {
            Command::new("shutdown").args(["/s", "/t", "0"]).spawn()?;
        }
        PowerAction::Restart => {
            Command::new("shutdown").args(["/r", "/t", "0"]).spawn()?;
        }
        PowerAction::Sleep => unsafe {
            if !windows::Win32::System::Power::SetSuspendState(false, false, false) {
                anyhow::bail!("sospensione non riuscita");
            }
        },
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn power(action: PowerAction) -> Result<()> {
    eprintln!("power action {action:?} ignored outside Windows");
    Ok(())
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
