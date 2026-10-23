import { AnimatePresence, motion } from "motion/react";
import { useRef } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { ensureFocus, focusElement } from "../input/focus";
import type { PowerAction } from "../lib/types";
import { useApp } from "../store/app";
import { useIdle } from "../store/idle";
import { Glyph, useScreenFocus } from "./common";

const VERB: Record<PowerAction, string> = {
  shutdown: "Spegnimento",
  restart: "Riavvio",
  sleep: "Sospensione",
  restartToDesktop: "Riavvio in modalità desktop",
  hibernate: "Ibernazione",
  lock: "Blocco",
  logoff: "Disconnessione",
  updateRestart: "Aggiornamento e riavvio",
  updateShutdown: "Aggiornamento e spegnimento",
};

/** The last minute before a scheduled power action, with a way out. */
export function PowerCountdown() {
  const countdown = useIdle((s) => s.countdown);
  const focusBefore = useRef<HTMLElement | null>(null);
  if (countdown && !focusBefore.current) focusBefore.current = document.activeElement as HTMLElement | null;

  // The warning takes the focus: give it back to where the user was.
  const restore = () => {
    const el = focusBefore.current;
    focusBefore.current = null;
    if (el?.isConnected && el !== document.body) focusElement(el);
    else ensureFocus();
  };

  return (
    <AnimatePresence onExitComplete={restore}>
      {countdown && <Warning key="countdown" action={countdown.action} left={countdown.secondsLeft} />}
    </AnimatePresence>
  );
}

function Warning({ action, left }: { action: PowerAction; left: number }) {
  const cancel = useIdle((s) => s.cancelPower);
  const paused = useApp((s) => s.suspended.length);
  useScreenFocus();
  // Any button cancels, as the text says.
  useAction((_, repeat) => {
    if (!repeat) void cancel();
    return true;
  }, Priority.BLOCKING);
  const closes = action !== "sleep" && paused > 0;

  return (
    <motion.div
      className="dialog-scrim countdown-scrim"
      data-nav-layer="countdown"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.18 }}
    >
      <motion.div className="dialog countdown" role="alertdialog" aria-live="assertive" initial={{ scale: 0.94, y: 12 }} animate={{ scale: 1, y: 0 }}>
        <span className="qm-label">Programmato</span>
        <h2 className="dialog-title">
          {VERB[action]} fra <span className="countdown-secs">{left}</span> s
        </h2>
        <p className="dialog-text">
          {closes ? `${paused === 1 ? "Il gioco in pausa verrà chiuso" : `I ${paused} giochi in pausa verranno chiusi`} · ` : ""}Premi un tasto per annullare
        </p>
        <div className="dialog-actions row">
          <button type="button" className="btn btn-primary" data-nav data-nav-autofocus onClick={() => void cancel()}>
            Annulla
          </button>
        </div>
        <div className="qm-hints">
          <span className="hint"><Glyph button="A" /> Annulla</span>
          <span className="hint"><Glyph button="B" /> Annulla</span>
        </div>
      </motion.div>
    </motion.div>
  );
}
