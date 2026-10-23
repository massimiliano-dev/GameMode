import { AnimatePresence } from "motion/react";
import { useCallback, useEffect, useRef, useState } from "react";

import { ActionSheet } from "../components/ActionSheet";
import { Row, Section } from "../components/SettingsRows";
import { Toggle, useHints, useScreenFocus } from "../components/common";
import { useAction } from "../input/dispatcher";
import { api } from "../lib/api";
import type { BtDevice, Radios, WifiNetwork } from "../lib/types";
import { useApp } from "../store/app";

/** Wi-Fi networks and Bluetooth devices, all reachable with the controller. */
export function Connections() {
  const toast = useApp((s) => s.toast);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const [radios, setRadios] = useState<Radios | null>(null);
  const [networks, setNetworks] = useState<WifiNetwork[] | null>(null);
  const [devices, setDevices] = useState<BtDevice[] | null>(null);
  const [discovering, setDiscovering] = useState(false);
  const [joining, setJoining] = useState<WifiNetwork | null>(null);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [device, setDevice] = useState<BtDevice | null>(null);

  const refresh = useCallback(async () => {
    const r = await api.getRadios().catch(() => ({ wifi: null, bluetooth: null }));
    setRadios(r);
    if (r.wifi) void api.wifiNetworks().then(setNetworks).catch(() => setNetworks([]));
    if (r.bluetooth) void api.bluetoothDevices(false).then(setDevices).catch(() => setDevices([]));
  }, []);

  useEffect(() => {
    setBackdrop(null);
    void refresh();
  }, [refresh, setBackdrop]);
  // A device connected or dropped (the top bar's counter moved): re-read the list.
  const btConnected = useApp((s) => s.system?.bluetooth.connected ?? 0);
  const btSeen = useRef(btConnected);
  // Skipped while a scan or a pair is running: they set the list themselves, and a
  // paired-only refresh landing after the scan would drop the discovered devices.
  useEffect(() => {
    if (btSeen.current === btConnected) return;
    btSeen.current = btConnected;
    if (discovering || busy) return;
    if (radios?.bluetooth) void api.bluetoothDevices(false).then(setDevices).catch(() => {});
  }, [btConnected, radios?.bluetooth, discovering, busy]);
  useScreenFocus(undefined, radios !== null);
  useHints([
    { button: "A", label: "Seleziona" },
    { button: "X", label: "Aggiorna" },
    { button: "B", label: "Indietro" },
  ]);
  useAction((action, repeat) => {
    if (action === "x" && !repeat) {
      setNetworks(null);
      void refresh();
      return true;
    }
  });

  const fail = (e: unknown) => toast(String(e), "error");

  const toggleRadio = async (bluetooth: boolean, on: boolean) => {
    try {
      await api.setRadio(bluetooth, on);
      await refresh();
    } catch (e) {
      fail(e);
    }
  };

  const connect = async (network: WifiNetwork, pw: string | null) => {
    setBusy(network.ssid);
    try {
      await api.wifiConnect(network.ssid, pw);
      toast(`Connesso a ${network.ssid}`);
      setJoining(null);
      setPassword("");
      setNetworks(await api.wifiNetworks());
    } catch (e) {
      fail(e);
    } finally {
      setBusy(null);
    }
  };

  const discover = async () => {
    setDiscovering(true);
    try {
      setDevices(await api.bluetoothDevices(true));
    } catch (e) {
      fail(e);
    } finally {
      setDiscovering(false);
    }
  };

  const pair = async (d: BtDevice) => {
    setBusy(d.id);
    try {
      await api.bluetoothPair(d.id);
      toast(`${d.name} abbinato`);
      setDevices(await api.bluetoothDevices(false));
    } catch (e) {
      fail(e);
    } finally {
      setBusy(null);
    }
  };

  const unpair = async (d: BtDevice) => {
    try {
      await api.bluetoothUnpair(d.id);
      toast(`${d.name} rimosso`);
      setDevices(await api.bluetoothDevices(false));
    } catch (e) {
      fail(e);
    }
  };

  if (!radios) return <div className="screen empty"><div className="spinner" /></div>;

  return (
    <div className="screen settings">
      <h1 className="screen-title">Wi-Fi e Bluetooth</h1>
      <div className="settings-scroll">
        {radios.wifi !== null && (
          <Section title="Wi-Fi">
            <Row label="Wi-Fi" autofocus onClick={() => void toggleRadio(false, !radios.wifi)} control={<Toggle on={!!radios.wifi} />} />
            {radios.wifi && networks === null && <p className="setting-hint">Cerco le reti…</p>}
            {radios.wifi &&
              networks?.map((n) =>
                joining?.ssid === n.ssid ? (
                  <div key={n.ssid} className="setting-row is-static">
                    <div className="setting-text">
                      <span className="setting-label">{n.ssid}</span>
                      <span className="setting-hint">Inserisci la password della rete</span>
                    </div>
                    <div className="setting-inline">
                      <input
                        className="text-input"
                        type="password"
                        data-nav
                        data-nav-id="wifi-password"
                        aria-label={`Password di ${n.ssid}`}
                        value={password}
                        onChange={(e) => setPassword(e.target.value)}
                        onKeyDown={(e) => e.key === "Enter" && void connect(n, password)}
                      />
                      <button type="button" className="btn" data-nav onClick={() => void connect(n, password)}>
                        {busy === n.ssid ? "Connessione…" : "Connetti"}
                      </button>
                    </div>
                  </div>
                ) : (
                  <Row
                    key={n.ssid}
                    label={n.ssid}
                    hint={[n.connected ? "Connesso" : null, n.secured ? "Protetta" : "Aperta", signal(n.bars)].filter(Boolean).join(" · ")}
                    onClick={() => {
                      if (n.connected) return;
                      if (!n.secured) return void connect(n, null);
                      setJoining(n);
                      setPassword("");
                      requestAnimationFrame(() => document.querySelector<HTMLElement>("[data-nav-id='wifi-password']")?.focus());
                    }}
                    control={<span className="setting-action">{busy === n.ssid ? "Connessione…" : n.connected ? "✓" : "Connetti"}</span>}
                  />
                ),
              )}
          </Section>
        )}

        {radios.bluetooth !== null && (
          <Section title="Bluetooth">
            <Row
              label="Bluetooth"
              onClick={() => void toggleRadio(true, !radios.bluetooth)}
              control={<Toggle on={!!radios.bluetooth} />}
            />
            {radios.bluetooth && (
              <>
                {devices?.map((d) => (
                  <Row
                    key={d.id}
                    label={d.name}
                    hint={d.paired ? (d.connected ? "Connesso" : "Abbinato") : "Disponibile per l'abbinamento"}
                    onClick={() => (d.paired ? setDevice(d) : void pair(d))}
                    control={<span className="setting-action">{busy === d.id ? "Abbinamento…" : d.paired ? "Opzioni" : "Abbina"}</span>}
                  />
                ))}
                <Row
                  label={discovering ? "Ricerca in corso…" : "Cerca dispositivi"}
                  hint="Metti il controller o le cuffie in modalità abbinamento (sul controller Xbox: tieni premuto il tasto di associazione)"
                  onClick={() => void discover()}
                />
              </>
            )}
          </Section>
        )}

        <Section title="Altro">
          <Row
            label="Apri le impostazioni di Windows"
            hint="Per reti nascoste, VPN o dispositivi che chiedono un PIN"
            onClick={() => void api.openLink("ms-settings:bluetooth")}
          />
        </Section>
      </div>
      <AnimatePresence>
        {device && (
          <ActionSheet
            key="device"
            title={device.name}
            onClose={() => setDevice(null)}
            actions={[{ label: "Rimuovi dispositivo", danger: true, onSelect: () => void unpair(device) }]}
          />
        )}
      </AnimatePresence>
    </div>
  );
}

function signal(bars: number) {
  return ["Segnale assente", "Segnale debole", "Segnale discreto", "Segnale buono", "Segnale ottimo"][Math.min(4, Math.max(0, bars))];
}
