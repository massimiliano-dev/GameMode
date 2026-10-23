import { useEffect, useState } from "react";

import { api, on } from "../lib/api";
import type { CompanionStatus, Settings } from "../lib/types";
import { useApp } from "../store/app";
import { Row, Section } from "./SettingsRows";
import { Toggle } from "./common";

/** Phone companion: remote, keyboard and game launcher from the phone's browser. */
export function CompanionSettings({ settings }: { settings: Settings }) {
  const saveSettings = useApp((s) => s.saveSettings);
  const toast = useApp((s) => s.toast);
  const [status, setStatus] = useState<CompanionStatus | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = () => void api.companionStatus().then(setStatus).catch(() => {});
  useEffect(() => {
    refresh();
    return on("companion://changed", refresh);
  }, []);

  const allowFirewall = async () => {
    setBusy(true);
    try {
      const ok = await api.companionFirewall();
      toast(ok ? "Il telefono può raggiungere GameMode" : "Senza la regola del firewall il telefono non raggiunge il PC", ok ? "info" : "error");
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const toggle = async () => {
    const enabled = !settings.companionEnabled;
    await saveSettings({ ...settings, companionEnabled: enabled });
    if (!enabled) return;
    const next = await api.companionStatus().catch(() => null);
    setStatus(next);
    if (next?.firewall === "missing") await allowFirewall();
  };

  const firewallMissing = settings.companionEnabled && status?.firewall === "missing";

  return (
    <Section title="Telefono">
      <Row
        label="Comando dal telefono"
        hint={
          settings.companionEnabled
            ? status?.error ?? (status?.running ? "Attivo: inquadra il codice QR con il telefono" : "In avvio…")
            : "Telecomando, tastiera e avvio dei giochi dal browser del telefono, sulla stessa rete Wi-Fi"
        }
        onClick={() => void toggle()}
        control={<Toggle on={settings.companionEnabled} />}
      />
      {firewallMissing && (
        <Row
          label={busy ? "Attendo la conferma di Windows…" : "Consenti nel firewall di Windows"}
          hint="Serve una volta sola: Windows chiede conferma (UAC) e GameMode aggiunge la regola per la porta 7373"
          onClick={() => !busy && void allowFirewall()}
          control={<span className="setting-action">Consenti</span>}
        />
      )}
      {settings.companionEnabled && status?.running && (
        <div className="setting-row is-static companion">
          {status.qrSvg ? (
            <div className="companion-qr" dangerouslySetInnerHTML={{ __html: status.qrSvg }} />
          ) : (
            <div className="companion-qr is-empty" />
          )}
          <div className="setting-text">
            <span className="setting-label">{status.address ?? "Nessuna rete: collega il PC al Wi-Fi o alla LAN"}</span>
            {status.pin && (
              <span className="setting-hint">
                Oppure apri l'indirizzo sul telefono e scrivi il codice <span className="mono companion-pin">{status.pin}</span>
              </span>
            )}
          </div>
        </div>
      )}
      <p className="setting-hint settings-note">
        Funziona solo con il telefono sulla stessa rete del PC, in chiaro (HTTP) e senza notifiche a pagina chiusa. Il
        testo scritto sul telefono finisce nel campo selezionato, anche nelle finestre dei negozi; i giochi con anti-cheat
        possono ignorarlo.
      </p>
    </Section>
  );
}
