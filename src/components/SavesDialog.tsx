import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { ensureFocus } from "../input/focus";
import { api } from "../lib/api";
import type { BackupInfo, Game, SaveInfo } from "../lib/types";
import { useApp } from "../store/app";
import { ActionSheet } from "./ActionSheet";
import { ConfirmDialog } from "./ConfirmDialog";
import { useScreenFocus } from "./common";

const dateFmt = new Intl.DateTimeFormat("it", { day: "numeric", month: "short", year: "numeric", hour: "2-digit", minute: "2-digit" });

function size(bytes: number) {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1).replace(".", ",")} MB`;
}

/** Save locations and backups of one game. */
export function SavesDialog({ game, onClose }: { game: Game; onClose: () => void }) {
  const toast = useApp((s) => s.toast);
  const [info, setInfo] = useState<SaveInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<BackupInfo | null>(null);
  const [confirm, setConfirm] = useState<BackupInfo | null>(null);

  const load = useCallback(() => {
    api
      .saveInfo(game.id)
      .then(setInfo)
      .catch((e) => setError(String(e)));
  }, [game.id]);

  useEffect(load, [load]);
  useScreenFocus(undefined, info !== null || error !== null);
  useAction((action) => {
    if (action === "back") {
      onClose();
      return true;
    }
    return !["up", "down", "left", "right", "confirm"].includes(action);
  }, Priority.BLOCKING, !selected && !confirm);

  const backupNow = async () => {
    setBusy(true);
    try {
      await api.backupGame(game.id);
      toast("Backup completato");
      load();
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  const restore = async (b: BackupInfo) => {
    setBusy(true);
    try {
      await api.restoreBackup(game.id, b.id);
      toast("Salvataggi ripristinati. Lo stato precedente è tra i backup automatici");
      load();
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <motion.div className="dialog-scrim" data-nav-layer="saves" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
      <motion.div className="dialog saves-dialog" role="dialog" aria-labelledby="saves-title" initial={{ scale: 0.95 }} animate={{ scale: 1 }}>
        <h2 id="saves-title" className="dialog-title">
          Salvataggi di {game.name}
        </h2>
        {!info && !error && <p className="dialog-text">Cerco i salvataggi… la prima volta scarico l'elenco dei percorsi, può volerci qualche secondo.</p>}
        {error && <p className="dialog-text">{error}</p>}
        {info && !info.known && (
          <p className="dialog-text">
            Questo gioco non è nel database dei percorsi dei salvataggi. Molti giochi recenti li tengono comunque nel cloud dello store.
          </p>
        )}
        {info?.known && (
          <>
            <p className="dialog-text">
              {info.locations.length
                ? `Trovati in: ${info.locations.join(" · ")}`
                : "Nessun salvataggio sul PC: forse non ci hai ancora giocato, o sono solo nel cloud."}
            </p>
            <div className="settings-list dialog-list">
              {info.locations.length > 0 && (
                <button type="button" className="setting-row" data-nav data-nav-autofocus onClick={() => void backupNow()}>
                  <span className="setting-text">
                    <span className="setting-label">{busy ? "Backup in corso…" : "Esegui backup ora"}</span>
                    <span className="setting-hint mono">{info.backupDir}</span>
                  </span>
                </button>
              )}
              {info.backups.map((b) => (
                <button key={b.id} type="button" className="setting-row" data-nav onClick={() => setSelected(b)}>
                  <span className="setting-text">
                    <span className="setting-label">{dateFmt.format(b.created * 1000)}</span>
                    <span className="setting-hint">
                      {size(b.bytes)}
                      {b.automatic ? " · automatico, prima di un ripristino" : ""}
                    </span>
                  </span>
                  <span className="setting-action">Opzioni</span>
                </button>
              ))}
            </div>
          </>
        )}
        <div className="button-row dialog-actions">
          <button type="button" className="btn btn-primary" data-nav data-nav-autofocus={!info?.locations.length || undefined} onClick={onClose}>
            Chiudi
          </button>
        </div>
      </motion.div>
      <AnimatePresence onExitComplete={ensureFocus}>
        {selected && (
          <ActionSheet
            key="backup"
            title={dateFmt.format(selected.created * 1000)}
            onClose={() => setSelected(null)}
            actions={[
              { label: "Ripristina questo backup", hint: "Sostituisce i salvataggi attuali", onSelect: () => setConfirm(selected) },
              {
                label: "Elimina backup",
                danger: true,
                onSelect: () =>
                  void api
                    .deleteBackup(game.id, selected.id)
                    .then(load)
                    .catch((e) => toast(String(e), "error")),
              },
            ]}
          />
        )}
        {confirm && (
          <ConfirmDialog
            key="restore"
            title="Ripristinare questo backup?"
            text="I salvataggi attuali vengono sostituiti. Prima ne faccio una copia automatica, così puoi tornare indietro."
            confirmLabel="Ripristina"
            onCancel={() => setConfirm(null)}
            onConfirm={() => {
              const b = confirm;
              setConfirm(null);
              void restore(b);
            }}
          />
        )}
      </AnimatePresence>
    </motion.div>
  );
}
