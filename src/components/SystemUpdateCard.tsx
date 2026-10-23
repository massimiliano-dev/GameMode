import { AnimatePresence } from "motion/react";
import { useState } from "react";

import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import type { PowerAction, SystemUpdates } from "../lib/types";
import { systemUpdateReady, useApp } from "../store/app";
import { ActionSheet } from "./ActionSheet";
import { ConfirmDialog } from "./ConfirmDialog";

/** Home row for a Windows update a restart would install. Nothing shows
 * until Windows has everything staged: the couch never sees a download.
 * The options open in `SystemUpdateSheet`, which Home mounts outside the
 * masked `.rows` (a mask clips fixed descendants too). */
export function SystemUpdateRow({ onFocus, onSelect }: { onFocus: () => void; onSelect: () => void }) {
  const update = useApp((s) => s.systemUpdate);
  if (!update || !systemUpdateReady(update)) return null;

  return (
    <section className="row" data-nav-group="row:system">
      <h2 className="row-title">Sistema</h2>
      <div className="row-scroll">
        <button type="button" className="system-card" data-nav data-nav-id="system:update" onFocus={onFocus} onClick={onSelect}>
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
    </section>
  );
}

/** Options for the update card. "Più tardi" comes first so a stray A never
 * restarts the PC, and both power actions still ask for confirmation. */
export function SystemUpdateSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const paused = useApp((s) => s.suspended.length);
  const toast = useApp((s) => s.toast);
  const [confirm, setConfirm] = useState<PowerAction | null>(null);
  const pausedNote = paused ? (paused === 1 ? " Il gioco in pausa viene chiuso." : ` I ${paused} giochi in pausa vengono chiusi.`) : "";

  return (
    <AnimatePresence>
      {open && (
        <ActionSheet
          key="system-update"
          title="Aggiornamento di Windows"
          onClose={onClose}
          actions={[
            { label: "Più tardi", hint: "Resta qui finché non riavvii", onSelect: onClose },
            { label: "Aggiorna e riavvia", hint: "Il PC si riavvia e installa l'aggiornamento", onSelect: () => setConfirm("updateRestart") },
            { label: "Aggiorna e spegni", hint: "Installa l'aggiornamento e spegne il PC", danger: true, onSelect: () => setConfirm("updateShutdown") },
          ]}
        />
      )}
      {confirm && (
        <ConfirmDialog
          key="system-update-confirm"
          title={confirm === "updateRestart" ? "Riavviare e aggiornare?" : "Spegnere e aggiornare?"}
          text={`Windows installa l'aggiornamento: può volerci qualche minuto e il PC può riavviarsi più volte.${pausedNote}`}
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
