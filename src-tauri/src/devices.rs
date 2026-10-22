//! Wi-Fi and Bluetooth from the controller: radio switches, joining a
//! network, pairing a controller or headset. Blocking (WinRT calls are
//! awaited with `join`): call from a blocking task.

use serde::Serialize;

#[derive(Serialize, Clone, Copy, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Radios {
    /// `None` when the PC has no such radio.
    pub wifi: Option<bool>,
    pub bluetooth: Option<bool>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WifiNetwork {
    pub ssid: String,
    /// 0–4.
    pub bars: u8,
    pub secured: bool,
    pub connected: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BtDevice {
    pub id: String,
    pub name: String,
    pub paired: bool,
    pub connected: bool,
}

/// Strongest entry per SSID, connected first, then by signal.
pub fn dedupe_networks(list: Vec<WifiNetwork>) -> Vec<WifiNetwork> {
    let mut out: Vec<WifiNetwork> = Vec::new();
    for n in list.into_iter().filter(|n| !n.ssid.trim().is_empty()) {
        match out.iter_mut().find(|o| o.ssid == n.ssid) {
            Some(o) => {
                o.bars = o.bars.max(n.bars);
                o.connected |= n.connected;
            }
            None => out.push(n),
        }
    }
    out.sort_by(|a, b| b.connected.cmp(&a.connected).then(b.bars.cmp(&a.bars)).then(a.ssid.cmp(&b.ssid)));
    out
}

#[cfg(windows)]
mod imp {
    use anyhow::{anyhow, bail, Result};
    use windows::core::HSTRING;
    use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
    use windows::Devices::Enumeration::{DeviceInformation, DevicePairingResultStatus, DeviceUnpairingResultStatus};
    use windows::Devices::Radios::{Radio, RadioKind, RadioState};
    use windows::Devices::WiFi::{WiFiAdapter, WiFiAvailableNetwork, WiFiConnectionStatus, WiFiReconnectionKind};
    use windows::Networking::Connectivity::NetworkAuthenticationType;
    use windows::Security::Credentials::PasswordCredential;

    use super::{BtDevice, Radios, WifiNetwork};

    pub fn radios() -> Result<Radios> {
        let mut out = Radios::default();
        for radio in Radio::GetRadiosAsync()?.join()? {
            let on = radio.State()? == RadioState::On;
            match radio.Kind()? {
                RadioKind::WiFi => out.wifi = Some(on),
                RadioKind::Bluetooth => out.bluetooth = Some(on),
                _ => {}
            }
        }
        Ok(out)
    }

    pub fn set_radio(bluetooth: bool, on: bool) -> Result<()> {
        Radio::RequestAccessAsync()?.join()?;
        let kind = if bluetooth { RadioKind::Bluetooth } else { RadioKind::WiFi };
        let state = if on { RadioState::On } else { RadioState::Off };
        for radio in Radio::GetRadiosAsync()?.join()? {
            if radio.Kind()? == kind {
                radio.SetStateAsync(state)?.join()?;
                return Ok(());
            }
        }
        bail!("radio non presente")
    }

    fn adapter() -> Result<WiFiAdapter> {
        WiFiAdapter::RequestAccessAsync()?.join()?;
        let adapters = WiFiAdapter::FindAllAdaptersAsync()?.join()?;
        if adapters.Size()? == 0 {
            bail!("nessuna scheda Wi-Fi");
        }
        Ok(adapters.GetAt(0)?)
    }

    fn scan(adapter: &WiFiAdapter) -> Result<Vec<WiFiAvailableNetwork>> {
        adapter.ScanAsync()?.join()?;
        Ok(adapter.NetworkReport()?.AvailableNetworks()?.into_iter().collect())
    }

    fn connected_ssid(adapter: &WiFiAdapter) -> Option<String> {
        let profile = adapter.NetworkAdapter().ok()?.GetConnectedProfileAsync().ok()?.join().ok()?;
        Some(profile.ProfileName().ok()?.to_string_lossy())
    }

    pub fn wifi_scan() -> Result<Vec<WifiNetwork>> {
        let adapter = adapter()?;
        let connected = connected_ssid(&adapter);
        let list = scan(&adapter)?
            .into_iter()
            .filter_map(|n| {
                let ssid = n.Ssid().ok()?.to_string_lossy();
                let auth = n.SecuritySettings().and_then(|s| s.NetworkAuthenticationType()).ok();
                Some(WifiNetwork {
                    connected: connected.as_deref() == Some(ssid.as_str()),
                    bars: n.SignalBars().unwrap_or(0),
                    secured: !matches!(auth, Some(NetworkAuthenticationType::Open80211) | Some(NetworkAuthenticationType::None)),
                    ssid,
                })
            })
            .collect();
        Ok(super::dedupe_networks(list))
    }

    pub fn wifi_connect(ssid: &str, password: Option<&str>) -> Result<()> {
        let adapter = adapter()?;
        let network = scan(&adapter)?
            .into_iter()
            .find(|n| n.Ssid().is_ok_and(|s| s.to_string_lossy() == ssid))
            .ok_or_else(|| anyhow!("rete {ssid} non trovata"))?;
        let result = match password.filter(|p| !p.is_empty()) {
            Some(pw) => {
                let credential = PasswordCredential::new()?;
                credential.SetPassword(&HSTRING::from(pw))?;
                adapter.ConnectWithPasswordCredentialAsync(&network, WiFiReconnectionKind::Automatic, &credential)?.join()?
            }
            None => adapter.ConnectAsync(&network, WiFiReconnectionKind::Automatic)?.join()?,
        };
        match result.ConnectionStatus()? {
            WiFiConnectionStatus::Success => Ok(()),
            WiFiConnectionStatus::InvalidCredential => bail!("password sbagliata"),
            WiFiConnectionStatus::Timeout => bail!("la rete non risponde"),
            _ => bail!("connessione non riuscita"),
        }
    }

    fn find(selector: windows::core::Result<HSTRING>) -> Vec<DeviceInformation> {
        selector
            .and_then(|s| DeviceInformation::FindAllAsyncAqsFilter(&s)?.join())
            .map(|c| c.into_iter().collect())
            .unwrap_or_default()
    }

    fn is_connected(id: &HSTRING) -> bool {
        let classic = BluetoothDevice::FromIdAsync(id)
            .and_then(|op| op.join())
            .and_then(|d| d.ConnectionStatus())
            .is_ok_and(|s| s == BluetoothConnectionStatus::Connected);
        classic
            || BluetoothLEDevice::FromIdAsync(id)
                .and_then(|op| op.join())
                .and_then(|d| d.ConnectionStatus())
                .is_ok_and(|s| s == BluetoothConnectionStatus::Connected)
    }

    pub fn bt_devices(discover: bool) -> Result<Vec<BtDevice>> {
        let mut infos = find(BluetoothDevice::GetDeviceSelectorFromPairingState(true));
        infos.extend(find(BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)));
        if discover {
            // An inquiry scan: takes a few seconds and finds devices in pairing mode.
            infos.extend(find(BluetoothDevice::GetDeviceSelectorFromPairingState(false)));
            infos.extend(find(BluetoothLEDevice::GetDeviceSelectorFromPairingState(false)));
        }
        let mut out: Vec<BtDevice> = Vec::new();
        for info in infos {
            let (Ok(id), Ok(name)) = (info.Id(), info.Name()) else { continue };
            let name = name.to_string_lossy();
            if name.trim().is_empty() || out.iter().any(|d| d.id == id.to_string_lossy()) {
                continue;
            }
            let paired = info.Pairing().and_then(|p| p.IsPaired()).unwrap_or(false);
            out.push(BtDevice { connected: paired && is_connected(&id), id: id.to_string_lossy(), name, paired });
        }
        out.sort_by(|a, b| b.paired.cmp(&a.paired).then(b.connected.cmp(&a.connected)).then(a.name.cmp(&b.name)));
        Ok(out)
    }

    pub fn bt_pair(id: &str) -> Result<()> {
        let info = DeviceInformation::CreateFromIdAsync(&HSTRING::from(id))?.join()?;
        match info.Pairing()?.PairAsync()?.join()?.Status()? {
            DevicePairingResultStatus::Paired | DevicePairingResultStatus::AlreadyPaired => Ok(()),
            status => bail!("abbinamento non riuscito (codice {}): prova dalle impostazioni Bluetooth di Windows", status.0),
        }
    }

    pub fn bt_unpair(id: &str) -> Result<()> {
        let info = DeviceInformation::CreateFromIdAsync(&HSTRING::from(id))?.join()?;
        match info.Pairing()?.UnpairAsync()?.join()?.Status()? {
            DeviceUnpairingResultStatus::Unpaired | DeviceUnpairingResultStatus::AlreadyUnpaired => Ok(()),
            status => bail!("rimozione non riuscita (codice {})", status.0),
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::Mutex;

    use anyhow::{bail, Result};

    use super::{BtDevice, Radios, WifiNetwork};

    static STATE: Mutex<(bool, bool, Option<String>, Vec<String>)> =
        Mutex::new((true, true, None, Vec::new()));

    pub fn radios() -> Result<Radios> {
        let s = STATE.lock().unwrap();
        Ok(Radios { wifi: Some(s.0), bluetooth: Some(s.1) })
    }

    pub fn set_radio(bluetooth: bool, on: bool) -> Result<()> {
        let mut s = STATE.lock().unwrap();
        if bluetooth { s.1 = on } else { s.0 = on }
        Ok(())
    }

    pub fn wifi_scan() -> Result<Vec<WifiNetwork>> {
        let connected = STATE.lock().unwrap().2.clone().unwrap_or_else(|| "Casa".into());
        Ok(super::dedupe_networks(
            [("Casa", 4, true), ("Casa-5G", 3, true), ("Ospiti", 2, false), ("Vicino", 1, true)]
                .into_iter()
                .map(|(ssid, bars, secured)| WifiNetwork { ssid: ssid.into(), bars, secured, connected: ssid == connected })
                .collect(),
        ))
    }

    pub fn wifi_connect(ssid: &str, password: Option<&str>) -> Result<()> {
        if ssid != "Ospiti" && password.is_none_or(|p| p.len() < 8) {
            bail!("password sbagliata");
        }
        STATE.lock().unwrap().2 = Some(ssid.into());
        Ok(())
    }

    pub fn bt_devices(discover: bool) -> Result<Vec<BtDevice>> {
        let paired = STATE.lock().unwrap().3.clone();
        let mut out = vec![BtDevice { id: "pad".into(), name: "Xbox Wireless Controller".into(), paired: true, connected: true }];
        for (id, name) in [("buds", "Cuffie Bluetooth"), ("pad2", "DualSense Wireless Controller")] {
            let is_paired = paired.iter().any(|p| p == id);
            if is_paired || discover {
                out.push(BtDevice { id: id.into(), name: name.into(), paired: is_paired, connected: false });
            }
        }
        Ok(out)
    }

    pub fn bt_pair(id: &str) -> Result<()> {
        STATE.lock().unwrap().3.push(id.into());
        Ok(())
    }

    pub fn bt_unpair(id: &str) -> Result<()> {
        STATE.lock().unwrap().3.retain(|p| p != id);
        Ok(())
    }
}

pub use imp::{bt_devices, bt_pair, bt_unpair, radios, set_radio, wifi_connect, wifi_scan};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_strongest_entry_per_network() {
        let n = |ssid: &str, bars, connected| WifiNetwork { ssid: ssid.into(), bars, secured: true, connected };
        let list = dedupe_networks(vec![n("A", 1, false), n("B", 4, false), n("A", 3, false), n("", 4, false), n("C", 1, true)]);
        let ssids: Vec<_> = list.iter().map(|n| (n.ssid.as_str(), n.bars)).collect();
        assert_eq!(ssids, [("C", 1), ("B", 4), ("A", 3)]);
    }
}
