//! "TV mode": the display mode GameMode runs in and the audio output it
//! switches to. Display changes use `CDS_FULLSCREEN`, so Windows restores the
//! desktop mode by itself when GameMode exits.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh: u32,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioOutput {
    pub id: String,
    pub name: String,
    pub default: bool,
}

/// Distinct modes, largest and fastest first.
pub fn sorted(mut modes: Vec<DisplayMode>) -> Vec<DisplayMode> {
    modes.sort_by(|a, b| (b.width * b.height, b.refresh).cmp(&(a.width * a.height, a.refresh)));
    modes.dedup();
    modes
}

#[cfg(windows)]
mod imp {
    use anyhow::{bail, Result};
    use windows::core::{GUID, HRESULT, PCWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Graphics::Gdi::{
        ChangeDisplaySettingsExW, EnumDisplaySettingsW, CDS_FULLSCREEN, CDS_TYPE, DEVMODEW, DISP_CHANGE_SUCCESSFUL,
        DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
    };
    use windows::Win32::Media::Audio::{
        eCommunications, eConsole, eMultimedia, eRender, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
    use windows::core::{IUnknown, Interface};

    use super::{AudioOutput, DisplayMode};

    fn devmode() -> DEVMODEW {
        DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() }
    }

    fn mode_of(dm: &DEVMODEW) -> DisplayMode {
        DisplayMode { width: dm.dmPelsWidth, height: dm.dmPelsHeight, refresh: dm.dmDisplayFrequency }
    }

    pub fn modes() -> Vec<DisplayMode> {
        let mut out = Vec::new();
        let mut i = 0;
        loop {
            let mut dm = devmode();
            if !unsafe { EnumDisplaySettingsW(PCWSTR::null(), ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm) }.as_bool() {
                break;
            }
            // Low-res and low-colour modes only clutter the list.
            if dm.dmBitsPerPel >= 32 && dm.dmPelsWidth >= 1280 && dm.dmDisplayFrequency > 1 {
                out.push(mode_of(&dm));
            }
            i += 1;
        }
        super::sorted(out)
    }

    pub fn current_mode() -> Option<DisplayMode> {
        let mut dm = devmode();
        unsafe { EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut dm) }.as_bool().then(|| mode_of(&dm))
    }

    pub fn apply_mode(mode: DisplayMode) -> Result<()> {
        let mut dm = devmode();
        dm.dmPelsWidth = mode.width;
        dm.dmPelsHeight = mode.height;
        dm.dmDisplayFrequency = mode.refresh;
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        let result = unsafe { ChangeDisplaySettingsExW(PCWSTR::null(), Some(&dm), None, CDS_FULLSCREEN, None) };
        if result != DISP_CHANGE_SUCCESSFUL {
            bail!("lo schermo non supporta {}×{} a {} Hz", mode.width, mode.height, mode.refresh);
        }
        Ok(())
    }

    pub fn reset_mode() {
        unsafe {
            let _ = ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None);
        }
    }

    fn enumerator() -> Result<IMMDeviceEnumerator> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            Ok(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?)
        }
    }

    fn take_pwstr(p: windows::core::PWSTR) -> String {
        unsafe {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        }
    }

    pub fn audio_outputs() -> Result<Vec<AudioOutput>> {
        unsafe {
            let enumerator = enumerator()?;
            let default_id = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .and_then(|d| d.GetId())
                .map(take_pwstr)
                .unwrap_or_default();
            let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let mut out = Vec::new();
            for i in 0..devices.GetCount()? {
                let device = devices.Item(i)?;
                let id = take_pwstr(device.GetId()?);
                let name = device
                    .OpenPropertyStore(STGM_READ)
                    .and_then(|store| store.GetValue(&PKEY_Device_FriendlyName))
                    .and_then(|value| PropVariantToStringAlloc(&value))
                    .map(take_pwstr)
                    .unwrap_or_else(|_| "Uscita audio".into());
                out.push(AudioOutput { default: id == default_id, id, name });
            }
            Ok(out)
        }
    }

    // IPolicyConfig: undocumented but stable since Windows 7; the only way to
    // change the default audio device. SetDefaultEndpoint is vtable slot 13.
    const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    type SetDefaultEndpoint = unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, i32) -> HRESULT;

    pub fn set_audio_output(id: &str) -> Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let unknown: IUnknown = CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL)?;
            let mut raw = std::ptr::null_mut();
            unknown.query(&IID_POLICY_CONFIG, &mut raw).ok()?;
            let config = IUnknown::from_raw(raw); // released on drop
            let vtable = *(config.as_raw() as *const *const usize);
            let set: SetDefaultEndpoint = std::mem::transmute(*vtable.add(13));
            let wide: Vec<u16> = id.encode_utf16().chain([0]).collect();
            for role in [eConsole, eMultimedia, eCommunications] {
                set(config.as_raw(), PCWSTR(wide.as_ptr()), role.0).ok()?;
            }
            Ok(())
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::Mutex;

    use anyhow::Result;

    use super::{AudioOutput, DisplayMode};

    static DEFAULT_AUDIO: Mutex<String> = Mutex::new(String::new());

    pub fn modes() -> Vec<DisplayMode> {
        super::sorted(vec![
            DisplayMode { width: 3840, height: 2160, refresh: 60 },
            DisplayMode { width: 2560, height: 1440, refresh: 144 },
            DisplayMode { width: 1920, height: 1080, refresh: 120 },
            DisplayMode { width: 1920, height: 1080, refresh: 60 },
        ])
    }

    pub fn current_mode() -> Option<DisplayMode> {
        Some(DisplayMode { width: 2560, height: 1440, refresh: 144 })
    }

    pub fn apply_mode(mode: DisplayMode) -> Result<()> {
        eprintln!("display mode {mode:?} ignored outside Windows");
        Ok(())
    }

    pub fn reset_mode() {}

    pub fn audio_outputs() -> Result<Vec<AudioOutput>> {
        let current = DEFAULT_AUDIO.lock().unwrap().clone();
        let current = if current.is_empty() { "speakers".to_string() } else { current };
        Ok([("speakers", "Altoparlanti"), ("tv", "TV (HDMI)"), ("headset", "Cuffie Xbox Wireless")]
            .into_iter()
            .map(|(id, name)| AudioOutput { id: id.into(), name: name.into(), default: id == current })
            .collect())
    }

    pub fn set_audio_output(id: &str) -> Result<()> {
        *DEFAULT_AUDIO.lock().unwrap() = id.to_string();
        Ok(())
    }
}

pub use imp::{apply_mode, audio_outputs, current_mode, modes, reset_mode, set_audio_output};

/// Puts the configured TV mode and audio output in place, if they differ.
pub fn apply_tv(mode: Option<DisplayMode>, audio: Option<&str>) {
    if let Some(mode) = mode {
        if current_mode() != Some(mode) {
            if let Err(err) = apply_mode(mode) {
                eprintln!("TV mode: {err:#}");
            }
        }
    }
    if let Some(id) = audio {
        let already = audio_outputs().map(|list| list.iter().any(|o| o.default && o.id == id)).unwrap_or(false);
        if !already {
            if let Err(err) = set_audio_output(id) {
                eprintln!("TV audio: {err:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_modes_biggest_and_fastest_first() {
        let m = |w, h, r| DisplayMode { width: w, height: h, refresh: r };
        assert_eq!(
            sorted(vec![m(1920, 1080, 60), m(3840, 2160, 60), m(1920, 1080, 120), m(1920, 1080, 60)]),
            vec![m(3840, 2160, 60), m(1920, 1080, 120), m(1920, 1080, 60)]
        );
    }
}
