import { AnimatePresence, motion } from "motion/react";

import { Priority, useAction } from "../input/dispatcher";
import type { PowerAction } from "../lib/types";
import { useIdle } from "../store/idle";
import { Glyph, useScreenFocus } from "./common";

const VERB: Record<PowerAction, string> = {
  shutdown: "Spegnimento",
  restart: "Riavvio",
  sleep: "Sospensione",
};

/** The last minute before a scheduled power action, with a way out. */
export function PowerCountdown() {
  const countdown = useIdle((s) => s.countdown);
  return <AnimatePresence>{countdown && <Warning key="countdown" action={countdown.action} left={countdown.secondsLeft} />}</AnimatePresence>;
}

function Warning({ action, left }: { action: PowerAction; left: number }) {
  const cancel = useIdle((s) => s.cancelPower);
  useScreenFocus();
  useAction((a, repeat) => {
    if (repeat) return true;
    if (a === "back" || a === "confirm") void cancel();
    return true;
  }, Priority.BLOCKING);

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
        <p className="dialog-text">Premi un tasto per annullare</p>
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
