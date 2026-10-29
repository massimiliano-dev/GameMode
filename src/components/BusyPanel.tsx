import { motion } from "motion/react";
import { useEffect, useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { useApp } from "../store/app";

/** After this long, say why it's taking a while. */
const SLOW_MS = 6000;

/**
 * GameMode is opening a store window (Steam's Big Picture may have to start
 * first): a spinner with what's happening and how to drive the window once
 * it's up. Input waits meanwhile.
 */
export function BusyPanel() {
  const busy = useApp((s) => s.busy);
  const [slow, setSlow] = useState(false);

  useEffect(() => {
    setSlow(false);
    if (!busy) return;
    const timer = setTimeout(() => setSlow(true), Math.max(0, busy.since + SLOW_MS - Date.now()));
    return () => clearTimeout(timer);
  }, [busy]);

  // Nothing to choose here: presses wait until the store window is up.
  useAction(() => true, Priority.BLOCKING, !!busy);

  if (!busy) return null;
  return (
    <motion.div
      className="busy-scrim"
      data-nav-layer="busy"
      role="status"
      aria-live="polite"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.15 }}
    >
      <div className="busy-card">
        <div className="spinner" />
        <p className="busy-title">{busy.title}…</p>
        <p className="busy-hint">{busy.hint}</p>
        {slow && <p className="busy-slow">Ci vuole qualche secondo in più: lo store si sta avviando.</p>}
      </div>
    </motion.div>
  );
}
