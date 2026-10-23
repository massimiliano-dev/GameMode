//! Push notifications for the system volume. Windows tells us through
//! `IAudioEndpointVolumeCallback` whenever the default output's volume or
//! mute changes, whoever changed it (volume keys, the Windows flyout, a
//! game, GameMode itself), and through `IMMNotificationClient` when the
//! default output itself changes, which needs the volume callback moved to
//! the new device. In console mode the keyboard's volume keys are claimed
//! with `RegisterHotKey`, so they keep working without Explorer.

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;

    use anyhow::Result;
    use tauri::AppHandle;
    use windows::core::{implement, PCWSTR};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::Media::Audio::Endpoints::{
        IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
    };
    use windows::Win32::Media::Audio::{
        eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
        MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
    };
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        RegisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

    use crate::osd;
    use crate::system::{self, Volume};

    /// The endpoint the callback is registered on. Kept alive here: the
    /// registration dies with the interface, and it has to be moved when
    /// the default output changes.
    struct VolumeRegistration {
        endpoint: IAudioEndpointVolume,
        callback: IAudioEndpointVolumeCallback,
    }
    /// Held only to keep the registration alive.
    struct DeviceRegistration {
        _enumerator: IMMDeviceEnumerator,
        _client: IMMNotificationClient,
    }
    // MMDevice objects are free-threaded; they are only used from MTA threads.
    unsafe impl Send for VolumeRegistration {}
    unsafe impl Send for DeviceRegistration {}

    static VOLUME_WATCH: Mutex<Option<VolumeRegistration>> = Mutex::new(None);
    static DEVICE_WATCH: Mutex<Option<DeviceRegistration>> = Mutex::new(None);

    /// Lets the new default output settle before asking it anything.
    const DEVICE_SETTLE: std::time::Duration = std::time::Duration::from_millis(300);

    #[implement(IAudioEndpointVolumeCallback)]
    struct VolumeWatcher {
        app: AppHandle,
    }

    impl IAudioEndpointVolumeCallback_Impl for VolumeWatcher_Impl {
        fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
            if data.is_null() {
                return Ok(());
            }
            // Called on a COM worker thread; only reads the notification.
            let data = unsafe { &*data };
            let volume = Volume { level: data.fMasterVolume, muted: data.bMuted.as_bool() };
            osd::on_volume(&self.app, volume, system::source_of(&data.guidEventContext));
            Ok(())
        }
    }

    #[implement(IMMNotificationClient)]
    struct DeviceWatcher {
        app: AppHandle,
    }

    impl IMMNotificationClient_Impl for DeviceWatcher_Impl {
        fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
            Ok(())
        }
        fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }
        fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
            Ok(())
        }
        fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
            if flow == eRender && role == eConsole {
                // Off the notification thread: re-registering talks to the
                // audio service, which is what is calling us.
                let app = self.app.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(DEVICE_SETTLE);
                    if let Err(err) = register_volume(&app) {
                        eprintln!("volume callback on new device: {err:#}");
                    }
                    if let (Some(name), Ok(volume)) = (default_output_name(), system::volume()) {
                        osd::on_device(&app, name, volume);
                    }
                });
            }
            Ok(())
        }
        fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
            Ok(())
        }
    }

    fn enumerator() -> Result<IMMDeviceEnumerator> {
        unsafe {
            // S_FALSE / RPC_E_CHANGED_MODE just mean COM is already set up.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            Ok(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?)
        }
    }

    /// Friendly name of the default output, e.g. "Cuffie (WH-1000XM4)".
    fn default_output_name() -> Option<String> {
        unsafe {
            let device = enumerator().ok()?.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let value = device.OpenPropertyStore(STGM_READ).ok()?.GetValue(&PKEY_Device_FriendlyName).ok()?;
            let p = PropVariantToStringAlloc(&value).ok()?;
            let name = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            Some(name).filter(|n| !n.is_empty())
        }
    }

    /// (Re)registers the volume callback on the current default output.
    fn register_volume(app: &AppHandle) -> Result<()> {
        let endpoint = system::endpoint()?;
        let callback: IAudioEndpointVolumeCallback = VolumeWatcher { app: app.clone() }.into();
        unsafe { endpoint.RegisterControlChangeNotify(&callback)? };
        let previous = VOLUME_WATCH.lock().unwrap().replace(VolumeRegistration { endpoint, callback });
        if let Some(old) = previous {
            unsafe {
                let _ = old.endpoint.UnregisterControlChangeNotify(&old.callback);
            }
        }
        Ok(())
    }

    fn register_device(app: &AppHandle) -> Result<()> {
        let enumerator = enumerator()?;
        let client: IMMNotificationClient = DeviceWatcher { app: app.clone() }.into();
        unsafe { enumerator.RegisterEndpointNotificationCallback(&client)? };
        *DEVICE_WATCH.lock().unwrap() = Some(DeviceRegistration { _enumerator: enumerator, _client: client });
        Ok(())
    }

    /// Starts listening for volume and default-output changes.
    pub fn watch(app: AppHandle) {
        std::thread::spawn(move || {
            if let Err(err) = register_volume(&app) {
                eprintln!("volume notifications: {err:#}");
            }
            if let Err(err) = register_device(&app) {
                eprintln!("audio device notifications: {err:#}");
            }
        });
    }

    const HOTKEY_UP: i32 = 1;
    const HOTKEY_DOWN: i32 = 2;
    const HOTKEY_MUTE: i32 = 3;

    /// Claims the keyboard's volume keys for as long as GameMode runs. Only
    /// for console mode: with Explorer around, Windows handles them itself.
    /// A thread with a message loop is required; keys another app already
    /// owns are skipped.
    pub fn hotkeys(app: AppHandle) {
        std::thread::spawn(move || unsafe {
            let mut claimed = false;
            // Volume keys repeat (holding one keeps stepping); mute must not,
            // or holding it would flip the state at the typematic rate.
            let keys = [
                (HOTKEY_UP, VK_VOLUME_UP, HOT_KEY_MODIFIERS(0)),
                (HOTKEY_DOWN, VK_VOLUME_DOWN, HOT_KEY_MODIFIERS(0)),
                (HOTKEY_MUTE, VK_VOLUME_MUTE, MOD_NOREPEAT),
            ];
            for (id, key, modifiers) in keys {
                match RegisterHotKey(None, id, modifiers, u32::from(key.0)) {
                    Ok(()) => claimed = true,
                    Err(err) => eprintln!("volume key {}: {err}", key.0),
                }
            }
            if !claimed {
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message != WM_HOTKEY {
                    continue;
                }
                match msg.wParam.0 as i32 {
                    HOTKEY_UP => osd::nudge(&app, osd::KEY_STEP),
                    HOTKEY_DOWN => osd::nudge(&app, -osd::KEY_STEP),
                    HOTKEY_MUTE => osd::toggle_mute(&app),
                    _ => {}
                }
            }
        });
    }
}

#[cfg(not(windows))]
mod imp {
    use tauri::AppHandle;

    pub fn watch(_app: AppHandle) {}

    pub fn hotkeys(_app: AppHandle) {}
}

/// Starts the volume watcher and, in console mode, the volume keys.
pub fn start(app: tauri::AppHandle) {
    imp::watch(app.clone());
    if crate::shell::status().active {
        imp::hotkeys(app);
    }
}
