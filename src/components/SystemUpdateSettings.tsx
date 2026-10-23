import { AnimatePresence } from "motion/react";
import { useState } from "react";

import { api } from "../lib/api";
import { formatBytes, formatLastPlayed } from "../lib/format";
import type { PowerAction, SystemUpdates } from "../lib/types";
import { systemUpdateReady, useApp } from "../store/app";
import { ActionSheet } from "./ActionSheet";
import { ConfirmDialog } from "./ConfirmDialog";
import { Row, Section } from "./SettingsRows";

/** Windows Update, read-only: what is pending and "update and restart". */
export function SystemUpdateSettings() {
  const update = useApp((s) => s.systemUpdate);
  const toast = useApp((s) => s.toast);
  const [checking, setChecking] = useState(false);
  const [sheet, setSheet] = useState(false);
  const [confirm, setConfirm] = useState<PowerAction | null>(null);

  const check = async () => {
    setChecking(true);
    try {
      const found = await api.systemUpdates(true);
      useApp.setState({ systemUpdate: found });
      if (found.state === "unknown") toast(`Windows Update non risponde: ${found.error ?? "riprova più tardi"}`, "error");
      // Online search failed: what shows comes from Windows' local cache.
      else if (found.error) toast(`Ricerca online non riuscita: ${found.error}`, "error");
      else if (found.state === "none") toast("Windows è aggiornato");
    } catch (e) {
      toast(`Controllo non riuscito: ${String(e)}`, "error");
    } finally {
      setChecking(false);
    }
  };

  const ready = systemUpdateReady(update);
  return (
    <Section title="Windows Update">
      <Row
        label={checking ? "Chiedo a Windows Update…" : "Cerca aggiornamenti di Windows"}
        hint={status(update)}
        onClick={() => !checking && void check()}
        control={update && update.state !== "none" && update.state !== "unknown" ? <span className="setting-action">{badge(update)}</span> : undefined}
      />
      {update?.items.map((item) => (
        <Row
          key={item.title}
          label={item.title}
          hint={item.downloaded ? "Scaricato" : "In attesa del download: ci pensa Windows"}
          onClick={() => {}}
          control={item.bytes ? <span className="setting-value">{formatBytes(item.bytes)}</span> : undefined}
        />
      ))}
      {ready && (
        <Row
          label="Aggiorna e riavvia"
          hint="Come dal menu Start: il PC si riavvia, installa e torna in GameMode"
          onClick={() => setSheet(true)}
          control={<span className="setting-action">Pronto</span>}
        />
      )}
      <AnimatePresence>
        {sheet && (
          <ActionSheet
            key="system-power"
            title="Aggiornamento di Windows"
            onClose={() => setSheet(false)}
            actions={[
              { label: "Aggiorna e riavvia", onSelect: () => { setSheet(false); setConfirm("updateRestart"); } },
              { label: "Aggiorna e spegni", onSelect: () => { setSheet(false); setConfirm("updateShutdown"); } },
            ]}
          />
        )}
        {confirm && (
          <ConfirmDialog
            key="system-confirm"
            title={confirm === "updateRestart" ? "Riavviare e aggiornare?" : "Spegnere e aggiornare?"}
            text="Windows installa l'aggiornamento: può volerci qualche minuto e il PC può riavviarsi più volte. I giochi in pausa vengono chiusi."
            confirmLabel={confirm === "updateRestart" ? "Riavvia" : "Spegni"}
            onCancel={() => setConfirm(null)}
            onConfirm={() => {
              const action = confirm;
              setConfirm(null);
              void api.power(action).catch((e) => toast(String(e), "error"));
            }}
          />
        )}
      </AnimatePresence>
    </Section>
  );
}

function badge(u: SystemUpdates): string {
  switch (u.state) {
    case "rebootRequired":
      return "Riavvio richiesto";
    case "ready":
      return "Pronto";
    case "available":
      return "In arrivo";
    default:
      return "";
  }
}

/** The hint under the search row. */
export function status(u: SystemUpdates | null): string {
  if (!u) return "GameMode controlla da solo ogni qualche ora. Scaricare e installare resta a Windows.";
  const when =
    (u.checkedAt ? `Ultimo controllo: ${formatLastPlayed(u.checkedAt).toLowerCase()}.` : "") +
    (u.error && u.state !== "unknown" ? " Ultima ricerca online non riuscita." : "");
  switch (u.state) {
    case "none":
      return `Windows è aggiornato. ${when}`.trim();
    case "available":
      return `Windows sta scaricando gli aggiornamenti in background: compariranno qui quando sono pronti. ${when}`.trim();
    case "ready":
      return `Tutto scaricato: al prossimo riavvio Windows lo installa. ${when}`.trim();
    case "rebootRequired":
      return `Un aggiornamento già installato aspetta un riavvio. ${when}`.trim();
    default:
      return u.error ? `Windows Update non risponde: ${u.error}` : "Windows Update non risponde";
  }
}
