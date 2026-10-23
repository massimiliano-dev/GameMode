import { useEffect, useRef, useState } from "react";

import { focusInitial } from "../input/focus";
import { api, on } from "../lib/api";
import type { CompanionStatus, Settings } from "../lib/types";
import { useApp } from "../store/app";
import { Row, Section } from "./SettingsRows";
import { Toggle } from "./common";

const TOGGLE_ID = "companion-toggle";

/** Phone companion: remote, keyboard and game launcher from the phone's browser. */
export function CompanionSettings({ settings }: { settings: Settings }) {
  const saveSettings = useApp((s) => s.saveSettings);
  const toast = useApp((s) => s.toast);
  const [status, setStatus] = useState<CompanionStatus | null>(null);
  const [busy, setBusy] = useState(false);
  // Reads run in parallel and take varying time (PowerShell on Windows):
  // only the most recent one may land.
  const latest = useRef(0);

  const refresh = () => {
    const id = ++latest.current;
    void api
      .companionStatus()
      .then((next) => {
        if (id === latest.current) setStatus(next);
      })
      .catch(() => {});
  };
  useEffect(() => {
    refresh();
    return on("companion://changed", refresh);
  }, []);

  /** Asks Windows for the firewall rule (UAC); true when it is in place. */
  const requestFirewall = async () => {
    try {
      const ok = await api.companionFirewall();
      toast(ok ? "Il telefono può raggiungere GameMode" : "Senza la regola del firewall il telefono non raggiunge il PC", ok ? "info" : "error");
      return ok;
    } catch (e) {
      toast(String(e), "error");
      return false;
    }
  };

  const allowFirewall = async () => {
    setBusy(true);
    await requestFirewall();
    setBusy(false);
    refresh();
  };

  const toggle = async () => {
    if (busy) return;
    const enabled = !settings.companionEnabled;
    setBusy(true);
    try {
      // The rule goes in before the server listens: otherwise Windows shows
      // its own alert, whose Cancel blocks GameMode for good.
      if (enabled) await requestFirewall();
      await saveSettings({ ...settings, companionEnabled: enabled });
    } finally {
      setBusy(false);
    }
  };

  const firewallMissing = settings.companionEnabled && status?.firewall === "missing";

  // The firewall row unmounts once the rule is in: if it had the focus, give
  // it back to the toggle instead of the top of the tab.
  const hadFirewallRow = useRef(false);
  useEffect(() => {
    if (firewallMissing) {
      hadFirewallRow.current = true;
      return;
    }
    if (!hadFirewallRow.current) return;
    hadFirewallRow.current = false;
    requestAnimationFrame(() => {
      const el = document.activeElement;
      if (!el || el === document.body) focusInitial(TOGGLE_ID);
    });
  }, [firewallMissing]);

  return (
    <Section title="Telefono">
      <Row
        label="Comando dal telefono"
        hint={
          busy && !settings.companionEnabled
            ? "Controllo il firewall di Windows…"
            : settings.companionEnabled
              ? status?.error ?? (status?.running ? "Attivo: inquadra il codice QR con il telefono" : "In avvio…")
              : "Telecomando, tastiera e avvio dei giochi dal browser del telefono, sulla stessa rete Wi-Fi"
        }
        navId={TOGGLE_ID}
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
