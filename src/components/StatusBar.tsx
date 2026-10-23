import type { BatteryInfo, NetworkInfo } from "../lib/types";
import { useApp } from "../store/app";

/** Updates that are actually moving (what the "N download" badge counts). */
export const isDownloading = (state: string) => state === "downloading" || state === "installing";

/** Network, PC battery, Bluetooth and running downloads, for the top bar. */
export function StatusCluster() {
  const system = useApp((s) => s.system);
  const downloads = useApp((s) => Object.values(s.gameUpdates).filter((u) => isDownloading(u.state)).length);
  if (!system && downloads === 0) return null;
  return (
    <span className="status-cluster">
      {downloads > 0 && (
        <span className="status-item status-downloads" title={downloads === 1 ? "1 download in corso" : `${downloads} download in corso`}>
          <svg className="status-icon" viewBox="0 0 24 24" aria-hidden>
            <path d="M12 3v11m0 0-4.5-4.5M12 14l4.5-4.5M4 18h16" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          <span className="status-count">{downloads}</span>
        </span>
      )}
      {system && system.bluetooth.on && (
        <span
          className="status-item status-bt"
          title={
            system.bluetooth.connected === 0
              ? "Bluetooth attivo"
              : system.bluetooth.connected === 1
                ? "Bluetooth: 1 dispositivo connesso"
                : `Bluetooth: ${system.bluetooth.connected} dispositivi connessi`
          }
        >
          <svg className="status-icon" viewBox="0 0 24 24" aria-hidden>
            <path d="M7 7l10 10-5 4V3l5 4L7 17" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
          {system.bluetooth.connected > 0 && <span className="status-count">{system.bluetooth.connected}</span>}
        </span>
      )}
      {system && <NetworkIcon network={system.network} />}
      {system?.battery && <PcBattery battery={system.battery} />}
    </span>
  );
}

function NetworkIcon({ network }: { network: NetworkInfo }) {
  const offline = network.kind === "none";
  const limited = !offline && !network.internet;
  const title = offline
    ? "Nessuna connessione"
    : `${network.kind === "wifi" ? (network.ssid ?? "Wi-Fi") : network.kind === "cellular" ? "Rete mobile" : "Ethernet"}${limited ? ": senza internet" : ""}`;
  const cls = `status-item status-net ${offline ? "is-offline" : ""} ${limited ? "is-limited" : ""}`;
  if (network.kind === "ethernet") {
    return (
      <span className={cls} title={title}>
        <svg className="status-icon" viewBox="0 0 24 24" aria-hidden>
          <path d="M4 9h16v8H4zM8 17v3h8v-3M9 13h2M13 13h2M12 4v5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinejoin="round" />
        </svg>
        {limited && <span className="status-mark">!</span>}
      </span>
    );
  }
  // Wi-Fi (or mobile data, drawn the same) as four arcs; offline keeps the
  // outline so the slot doesn't jump around.
  const bars = offline ? 0 : (network.bars ?? 4);
  return (
    <span className={cls} title={title}>
      <svg className="status-icon" viewBox="0 0 24 24" aria-hidden>
        <circle cx="12" cy="18.5" r="1.7" className={`status-arc ${bars > 0 ? "is-on" : ""}`} fill="currentColor" />
        {["M8.5 15.2a5 5 0 0 1 7 0", "M5.5 11.8a9.5 9.5 0 0 1 13 0", "M2.5 8.3a14 14 0 0 1 19 0"].map((d, i) => (
          <path key={d} d={d} className={`status-arc ${bars > i + 1 ? "is-on" : ""}`} fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" />
        ))}
        {offline && <path d="M4 20 20 4" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" />}
      </svg>
      {limited && <span className="status-mark">!</span>}
    </span>
  );
}

function PcBattery({ battery }: { battery: BatteryInfo }) {
  const low = !battery.plugged && battery.percent <= 10;
  const title = `Batteria del PC: ${battery.percent}%${battery.charging ? ", in carica" : battery.plugged ? ", alimentato" : ""}${battery.saver ? " (risparmio energetico)" : ""}`;
  return (
    <span className={`status-item status-pc-battery ${low ? "is-low" : ""} ${battery.saver ? "is-saver" : ""}`} title={title}>
      <svg className="status-icon status-battery" viewBox="0 0 28 16" aria-hidden>
        <rect x="1" y="2" width="22" height="12" rx="2.5" fill="none" stroke="currentColor" strokeWidth="1.8" />
        <rect x="24.5" y="5.5" width="2.5" height="5" rx="1" fill="currentColor" />
        <rect x="3.5" y="4.5" width={Math.max(1, (17 * Math.min(100, battery.percent)) / 100)} height="7" rx="1" className="status-battery-fill" />
        {battery.plugged && <path d="M13.5 2.5 9.5 9h3.5l-1.5 5 4.5-7h-3.5z" className="status-bolt" />}
      </svg>
      <span className="status-percent">{battery.percent}%</span>
    </span>
  );
}
