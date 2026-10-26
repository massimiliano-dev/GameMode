import { AnimatePresence } from "motion/react";
import { useCallback, useEffect, useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { ensureFocus } from "../input/focus";
import { api } from "../lib/api";
import type { CheatHit, CheatKind, CheatsStatus } from "../lib/types";
import { useApp } from "../store/app";
import { QmButton } from "../screens/QuickMenu";
import { ActionSheet } from "./ActionSheet";
import { Glyph } from "./common";

const KIND_LABEL: Record<CheatKind, string> = {
  int: "Numero intero",
  float: "Decimale",
  double: "Decimale (preciso)",
};
const KINDS: CheatKind[] = ["int", "float", "double"];
const KEYS = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "-", "0", ","];
/** Hits are listed by the backend once there are this few. */
const LISTED = 20;

const hex = (address: number) => `0x${address.toString(16).toUpperCase()}`;

/** Trucchi in the game overlay: find a value in the game's memory, then set or freeze it. */
export function CheatsPanel({ onClose }: { onClose: () => void }) {
  const toast = useApp((s) => s.toast);
  const [status, setStatus] = useState<CheatsStatus | null>(null);
  const [kind, setKind] = useState<CheatKind>("int");
  const [entry, setEntry] = useState("");
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<CheatHit | null>(null);

  const refresh = useCallback(() => {
    void api.cheatsStatus().then((s) => {
      setStatus(s);
      if (s.scan) setKind(s.scan.kind);
    });
  }, []);

  // Live values: the game keeps changing them.
  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 1000);
    return () => clearInterval(timer);
  }, [refresh]);

  const type = (key: string) => setEntry((e) => (key === "⌫" ? e.slice(0, -1) : e.length < 12 ? e + key : e));

  useAction((action, repeat) => {
    if (repeat && action !== "x") return false;
    if (action === "back") {
      onClose();
      return true;
    }
    if (action === "x") {
      type("⌫");
      return true;
    }
    if (action === "y") {
      setKind((k) => KINDS[(KINDS.indexOf(k) + 1) % KINDS.length]);
      return true;
    }
    return false;
  }, Priority.BLOCKING);

  const search = async (fresh: boolean) => {
    if (busy) return;
    if (!entry) return toast("Scrivi prima il valore che vedi nel gioco");
    setBusy(true);
    try {
      const scan = await api.cheatsSearch(entry, kind, fresh);
      setStatus((s) => (s ? { ...s, scan } : s));
      if (scan.count === 0) toast("Nessun risultato: controlla il valore o prova un altro tipo");
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setBusy(false);
      requestAnimationFrame(ensureFocus);
    }
  };

  const run = (action: Promise<void>, done: string) =>
    void action
      .then(() => {
        toast(done);
        refresh();
      })
      .catch((e) => toast(String(e), "error"));

  if (!status) return <p className="qm-sub">Un attimo…</p>;

  if (!status.available) {
    return (
      <div className="cheats">
        <span className="qm-label">Trucchi</span>
        <p className="cheats-note">{status.reason}</p>
        <div className="qm-list">
          <QmButton autofocus onClick={onClose}>
            Indietro
          </QmButton>
        </div>
      </div>
    );
  }

  const scan = status.scan;
  return (
    <div className="cheats">
      <span className="qm-label">Trucchi · solo giochi in singolo</span>
      <p className="cheats-note">
        {scan
          ? scan.count > LISTED
            ? `${scan.count.toLocaleString("it-IT")} risultati. Torna al gioco, fai cambiare il valore, poi scrivi il nuovo e premi Cerca di nuovo.`
            : "Scegli il risultato da modificare o bloccare."
          : "Scrivi il valore che vedi nel gioco (monete, vita, munizioni) e premi Nuova ricerca."}
      </p>

      <QmButton onClick={() => setKind((k) => KINDS[(KINDS.indexOf(k) + 1) % KINDS.length])}>
        Tipo: {KIND_LABEL[kind]}
      </QmButton>

      <div className="cheats-entry" aria-live="polite">
        {entry || <span className="cheats-placeholder">Valore</span>}
      </div>
      <div className="numpad" data-nav-group="numpad">
        {KEYS.map((key, i) => (
          <button key={key} type="button" className="qm-button numpad-key" data-nav data-nav-autofocus={i === 0 || undefined} onClick={() => type(key)}>
            {key}
          </button>
        ))}
      </div>

      <div className="qm-list">
        <QmButton onClick={() => void search(true)}>{busy ? "Ricerca…" : "Nuova ricerca"}</QmButton>
        {scan && scan.count > 0 && <QmButton onClick={() => void search(false)}>Cerca di nuovo</QmButton>}
      </div>

      {scan && scan.hits.length > 0 && (
        <div className="qm-list">
          {scan.hits.map((h) => (
            <QmButton key={h.address} onClick={() => setSelected(h)}>
              <span className="cheats-value">{h.value}</span>
              <span className="qm-sub">
                {hex(h.address)}
                {h.frozen ? " · bloccato" : ""}
              </span>
            </QmButton>
          ))}
        </div>
      )}

      {status.frozen.length > 0 && (
        <>
          <span className="qm-label">Bloccati · A per sbloccare</span>
          <div className="qm-list">
            {status.frozen.map((f) => (
              <QmButton key={f.address} onClick={() => run(api.cheatsUnfreeze(f.address), "Valore sbloccato")}>
                <span className="cheats-value">{f.value}</span>
                <span className="qm-sub">{hex(f.address)}</span>
              </QmButton>
            ))}
          </div>
        </>
      )}

      <div className="qm-hints">
        <span className="hint"><Glyph button="X" /> Cancella</span>
        <span className="hint"><Glyph button="Y" /> Tipo</span>
        <span className="hint"><Glyph button="B" /> Indietro</span>
      </div>

      <AnimatePresence onExitComplete={ensureFocus}>
        {selected && (
          <ActionSheet
            key="hit"
            title={`${selected.value} · ${hex(selected.address)}`}
            onClose={() => setSelected(null)}
            actions={[
              ...(entry
                ? [
                    { label: `Imposta a ${entry}`, onSelect: () => run(api.cheatsSet(selected.address, entry), `Valore impostato a ${entry}`) },
                    { label: `Blocca a ${entry}`, hint: "Resta fisso finché giochi", onSelect: () => run(api.cheatsFreeze(selected.address, entry), `Bloccato a ${entry}`) },
                  ]
                : []),
              {
                label: "Blocca al valore attuale",
                hint: "Vita o munizioni che non calano",
                onSelect: () => run(api.cheatsFreeze(selected.address, null), "Valore bloccato"),
              },
              ...(selected.frozen
                ? [{ label: "Sblocca", onSelect: () => run(api.cheatsUnfreeze(selected.address), "Valore sbloccato") }]
                : []),
            ]}
          />
        )}
      </AnimatePresence>
    </div>
  );
}
