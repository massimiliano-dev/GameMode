//! System status for the top bar: internet connection, the PC's own battery
//! (handhelds, laptops) and Bluetooth. Windows pushes changes through WinRT
//! events; each one is debounced and the result goes to the frontend as a
//! `system://status` event, only when something actually changed.
// Kinds and helpers below are only reached from Windows-only code paths.
#![cfg_attr(not(windows), allow(dead_code))]

use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NetworkKind {
    /// No connection at all.
    #[default]
    None,
    Wifi,
    Ethernet,
    /// Mobile data (a SIM or a tethered phone seen as WWAN).
    Cellular,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub kind: NetworkKind,
    /// Wi-Fi signal, 0–4; `None` on Ethernet or when Windows doesn't say.
    pub bars: Option<u8>,
    pub ssid: Option<String>,
    /// The internet is reachable, not just the local network.
    pub internet: bool,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
    /// 0–100.
    pub percent: u8,
    pub charging: bool,
    /// On mains power (charging or already full).
    pub plugged: bool,
    /// Windows' battery saver is on.
    pub saver: bool,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Bluetooth {
    /// `None` when the PC has no Bluetooth radio.
    pub on: Option<bool>,
    /// Devices connected right now (controllers, headsets…).
    pub connected: u32,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    pub network: Network,
    /// `None` on a desktop without a battery: nothing to show.
    pub battery: Option<Battery>,
    pub bluetooth: Bluetooth,
}

/// WinRT's connection profile reports 0–5 signal bars; the UI draws 4.
pub fn bars_of_five(bars: u8) -> u8 {
    (u16::from(bars.min(5)) * 4).div_ceil(5) as u8
}

/// Battery from Win32 `SYSTEM_POWER_STATUS` (fallback when the WinRT
/// PowerManager isn't available): `None` when there is no battery.
pub fn battery_from_win32(ac_line: u8, flag: u8, percent: u8) -> Option<Battery> {
    const NO_BATTERY: u8 = 128;
    const CHARGING: u8 = 8;
    const UNKNOWN: u8 = 255;
    if flag == UNKNOWN || flag & NO_BATTERY != 0 || percent == UNKNOWN {
        return None;
    }
    Some(Battery {
        percent: percent.min(100),
        charging: flag & CHARGING != 0,
        plugged: ac_line == 1,
        saver: false,
    })
}

#[cfg(windows)]
mod imp {
    use std::collections::HashSet;
    use std::sync::mpsc::{self, RecvTimeoutError, Sender};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use tauri::{AppHandle, Emitter};
    use windows::core::{IInspectable, Ref, HSTRING};
    use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
    use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationUpdate, DeviceWatcher};
    use windows::Devices::Radios::Radio;
    use windows::Foundation::{EventHandler, TypedEventHandler};
    use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation, NetworkStatusChangedEventHandler};
    use windows::System::Power::{BatteryStatus, EnergySaverStatus, PowerManager, PowerSupplyStatus};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    use super::{Battery, Bluetooth, Network, NetworkKind, SystemInfo};

    /// Events come in bursts (a Wi-Fi handshake fires several): wait for quiet.
    const DEBOUNCE: Duration = Duration::from_millis(1200);
    /// Signal strength changes without any event: look again now and then.
    const REFRESH: Duration = Duration::from_secs(30);

    /// Ids of the Bluetooth devices connected right now, kept by the watchers.
    static CONNECTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    /// Wakes the status thread; set once the thread runs.
    static POKE: OnceLock<Sender<()>> = OnceLock::new();

    fn poke() {
        if let Some(tx) = POKE.get() {
            let _ = tx.send(());
        }
    }

    fn network() -> Network {
        // Offline: Windows returns a null profile, which surfaces as an error.
        let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() else {
            return Network::default();
        };
        let level = profile.GetNetworkConnectivityLevel().unwrap_or(NetworkConnectivityLevel::None);
        let kind = if profile.IsWlanConnectionProfile().unwrap_or(false) {
            NetworkKind::Wifi
        } else if profile.IsWwanConnectionProfile().unwrap_or(false) {
            NetworkKind::Cellular
        } else {
            NetworkKind::Ethernet
        };
        let (ssid, bars) = if kind == NetworkKind::Wifi {
            let ssid = profile
                .WlanConnectionProfileDetails()
                .and_then(|d| d.GetConnectedSsid())
                .ok()
                .map(|s| s.to_string_lossy())
                .filter(|s| !s.trim().is_empty());
            // A null reference on some adapters: just no bars then.
            let bars = profile.GetSignalBars().and_then(|r| r.Value()).ok().map(super::bars_of_five);
            (ssid, bars)
        } else {
            (None, None)
        };
        Network { kind, bars, ssid, internet: level == NetworkConnectivityLevel::InternetAccess }
    }

    fn battery() -> Option<Battery> {
        match PowerManager::BatteryStatus() {
            Ok(BatteryStatus::NotPresent) => None,
            Ok(status) => {
                let percent = PowerManager::RemainingChargePercent().unwrap_or(0).clamp(0, 100) as u8;
                let supply = PowerManager::PowerSupplyStatus().unwrap_or(PowerSupplyStatus::NotPresent);
                let saver = PowerManager::EnergySaverStatus().is_ok_and(|s| s == EnergySaverStatus::On);
                Some(Battery { percent, charging: status == BatteryStatus::Charging, plugged: supply != PowerSupplyStatus::NotPresent, saver })
            }
            // PowerManager unavailable (unlikely): the classic Win32 call.
            Err(_) => {
                let mut status = SYSTEM_POWER_STATUS::default();
                unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
                super::battery_from_win32(status.ACLineStatus, status.BatteryFlag, status.BatteryLifePercent)
            }
        }
    }

    fn bluetooth() -> Bluetooth {
        let on = crate::devices::radios().ok().and_then(|r| r.bluetooth);
        let connected = CONNECTED.lock().unwrap().as_ref().map_or(0, |s| s.len()) as u32;
        Bluetooth { on, connected: if on == Some(false) { 0 } else { connected } }
    }

    pub fn snapshot() -> SystemInfo {
        SystemInfo { network: network(), battery: battery(), bluetooth: bluetooth() }
    }

    /// Watches Bluetooth devices that match `selector` (classic or LE), adding
    /// and removing their ids from `CONNECTED`.
    fn watch_bluetooth(selector: windows::core::Result<HSTRING>) -> windows::core::Result<()> {
        let watcher: DeviceWatcher = DeviceInformation::CreateWatcherAqsFilter(&selector?)?;
        watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(|_, info: Ref<DeviceInformation>| {
            if let Some(id) = info.ok().ok().and_then(|i| i.Id().ok()) {
                CONNECTED.lock().unwrap().get_or_insert_with(HashSet::new).insert(id.to_string_lossy());
                poke();
            }
            Ok(())
        }))?;
        watcher.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(|_, info: Ref<DeviceInformationUpdate>| {
            if let Some(id) = info.ok().ok().and_then(|i| i.Id().ok()) {
                CONNECTED.lock().unwrap().get_or_insert_with(HashSet::new).remove(&id.to_string_lossy());
                poke();
            }
            Ok(())
        }))?;
        watcher.EnumerationCompleted(&TypedEventHandler::<DeviceWatcher, IInspectable>::new(|_, _| {
            poke();
            Ok(())
        }))?;
        watcher.Start()?;
        // Events stop when the watcher is released: keep it for the whole run.
        std::mem::forget(watcher);
        Ok(())
    }

    fn subscribe() {
        if let Err(err) = NetworkInformation::NetworkStatusChanged(&NetworkStatusChangedEventHandler::new(|_| {
            poke();
            Ok(())
        })) {
            eprintln!("status: network events: {err}");
        }
        let on_power = || {
            EventHandler::<IInspectable>::new(|_, _| {
                poke();
                Ok(())
            })
        };
        let _ = PowerManager::BatteryStatusChanged(&on_power());
        let _ = PowerManager::RemainingChargePercentChanged(&on_power());
        let _ = PowerManager::PowerSupplyStatusChanged(&on_power());
        let _ = PowerManager::EnergySaverStatusChanged(&on_power());
        if let Ok(radios) = Radio::GetRadiosAsync().and_then(|op| op.join()) {
            for radio in radios {
                let ok = radio
                    .StateChanged(&TypedEventHandler::<Radio, IInspectable>::new(|_, _| {
                        poke();
                        Ok(())
                    }))
                    .is_ok();
                if ok {
                    std::mem::forget(radio);
                }
            }
        }
        for selector in [
            BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
            BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
        ] {
            if let Err(err) = watch_bluetooth(selector) {
                eprintln!("status: bluetooth watcher: {err}");
            }
        }
    }

    pub fn start(app: AppHandle) {
        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let (tx, rx) = mpsc::channel();
            let _ = POKE.set(tx);
            subscribe();
            let mut last: Option<SystemInfo> = None;
            loop {
                let current = snapshot();
                if last.as_ref() != Some(&current) {
                    let _ = app.emit("system://status", &current);
                    last = Some(current);
                }
                match rx.recv_timeout(REFRESH) {
                    Ok(()) => {
                        // Let the burst settle, then look once.
                        std::thread::sleep(DEBOUNCE);
                        while rx.try_recv().is_ok() {}
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
    }
}

#[cfg(not(windows))]
mod imp {
    use tauri::{AppHandle, Emitter};

    use super::{Battery, Bluetooth, Network, NetworkKind, SystemInfo};

    pub fn snapshot() -> SystemInfo {
        SystemInfo {
            network: Network { kind: NetworkKind::Wifi, bars: Some(3), ssid: Some("Casa".into()), internet: true },
            battery: Some(Battery { percent: 72, charging: false, plugged: false, saver: false }),
            bluetooth: Bluetooth { on: Some(true), connected: 1 },
        }
    }

    pub fn start(app: AppHandle) {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let _ = app.emit("system://status", &snapshot());
        });
    }
}

pub use imp::{snapshot, start};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_five_signal_bars_onto_four() {
        let mapped: Vec<u8> = (0..=6).map(bars_of_five).collect();
        assert_eq!(mapped, [0, 1, 2, 3, 4, 4, 4]);
    }

    #[test]
    fn reads_the_win32_power_status() {
        // Desktop: no battery, or everything unknown.
        assert_eq!(battery_from_win32(1, 128, 255), None);
        assert_eq!(battery_from_win32(255, 255, 255), None);
        assert_eq!(battery_from_win32(0, 1, 255), None);
        // Handheld on battery, then charging on mains.
        assert_eq!(battery_from_win32(0, 2, 45), Some(Battery { percent: 45, charging: false, plugged: false, saver: false }));
        assert_eq!(battery_from_win32(1, 8, 90), Some(Battery { percent: 90, charging: true, plugged: true, saver: false }));
        // Out-of-range percentages are clamped.
        assert_eq!(battery_from_win32(1, 0, 120).map(|b| b.percent), Some(100));
    }

    #[test]
    fn serializes_for_the_frontend() {
        let json = serde_json::to_value(snapshot_like()).unwrap();
        assert_eq!(json["network"]["kind"], "wifi");
        assert_eq!(json["network"]["bars"], 2);
        assert_eq!(json["battery"]["percent"], 20);
        assert_eq!(json["bluetooth"]["connected"], 2);
        let offline = serde_json::to_value(SystemInfo::default()).unwrap();
        assert_eq!(offline["network"]["kind"], "none");
        assert!(offline["battery"].is_null());
    }

    fn snapshot_like() -> SystemInfo {
        SystemInfo {
            network: Network { kind: NetworkKind::Wifi, bars: Some(2), ssid: Some("Casa".into()), internet: true },
            battery: Some(Battery { percent: 20, charging: true, plugged: true, saver: false }),
            bluetooth: Bluetooth { on: Some(true), connected: 2 },
        }
    }
}
