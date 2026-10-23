import { AnimatePresence } from "motion/react";
import { useState } from "react";

import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import type { PowerAction, SystemUpdates } from "../lib/types";
import { systemUpdateReady, useApp } from "../store/app";
import { ActionSheet } from "./ActionSheet";

/** Home row for a Windows update a restart would install. Nothing shows
 * until Windows has everything staged: the couch never sees a download. */
export function SystemUpdateRow({ onFocus }: { onFocus: () => void }) {
  const update = useApp((s) => s.systemUpdate);
  const paused = useApp((s) => s.suspended.length);
  const toast = useApp((s) => s.toast);
  const [sheet, setSheet] = useState(false);
  if (!update || !systemUpdateReady(update)) return null;

  const power = (action: PowerAction) => {
    setSheet(false);
    void api.power(action).catch((e) => toast(String(e), "error"));
  };
  const pausedNote = paused ? (paused === 1 ? "Il gioco in pausa viene chiuso" : `I ${paused} giochi in pausa vengono chiusi`) : undefined;

  return (
    <section className="row" data-nav-group="row:system">
      <h2 className="row-title">Sistema</h2>
      <div className="row-scroll">
        <button type="button" className="system-card" data-nav data-nav-id="system:update" onFocus={onFocus} onClick={() => setSheet(true)}>
          <span className="system-card-glyph" aria-hidden>
            ⟳
          </span>
          <span className="system-card-text">
            <span className="system-card-title">Aggiornamento di sistema pronto</span>
            <span className="system-card-sub">{summary(update)}</span>
          </span>
          <span className="playing-pill">Windows</span>
        </button>
      </div>
      <AnimatePresence>
        {sheet && (
          <ActionSheet
            key="system-update"
            title="Aggiornamento di Windows"
            onClose={() => setSheet(false)}
            actions={[
              { label: "Aggiorna e riavvia", hint: pausedNote ?? "Il PC si riavvia e installa l'aggiornamento", onSelect: () => power("updateRestart") },
              { label: "Aggiorna e spegni", hint: pausedNote ?? "Installa l'aggiornamento e spegne il PC", onSelect: () => power("updateShutdown") },
              { label: "Più tardi", hint: "Resta qui finché non riavvii", onSelect: () => setSheet(false) },
            ]}
          />
        )}
      </AnimatePresence>
    </section>
  );
}

/** One line for the card: what a restart would do. */
export function summary(u: SystemUpdates): string {
  if (u.state === "rebootRequired") return "Già installato: riavvia quando vuoi per completarlo";
  const n = u.items.length;
  const bytes = u.items.reduce((sum, i) => sum + i.bytes, 0);
  const size = bytes ? ` · ${formatBytes(bytes)}` : "";
  if (n === 1) return `Riavvia quando vuoi: Windows lo installa${size}`;
  return `${n} aggiornamenti scaricati${size} · riavvia quando vuoi`;
}
