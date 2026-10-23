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
    log::warn!("power action {action:?} ignored outside Windows");
    Ok(())
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
