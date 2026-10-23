import { motion } from "motion/react";

import { Priority, useAction } from "../input/dispatcher";
import { api } from "../lib/api";
import { formatPlaytime } from "../lib/format";
import { useApp } from "../store/app";
import { Cover } from "./GameCard";
import { Glyph, useClock } from "./common";

/** After this long, explain that the wait is normal. */
const SLOW_SECS = 20;

/** Shown while a store client is starting the game. */
export function LaunchOverlay({ onDismiss }: { onDismiss: () => void }) {
  const session = useApp((s) => s.session);
  const game = useApp((s) => s.games.find((g) => g.id === s.session?.gameId));
  const now = useClock(1000);

  useAction((action) => {
    if (action === "back") onDismiss();
    if (action === "x") void api.closeGame();
    if (action === "menu" || action === "guide") return false;
    return true;
  }, Priority.MODAL);

  if (!session || !game) return null;
  const waited = Math.max(0, Math.floor(now.getTime() / 1000) - session.startedAt);
  return (
    <motion.div
      className="launch-overlay"
      data-nav-layer="launch"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.3 }}
    >
      <motion.div
        className="launch-card"
        initial={{ scale: 0.92, y: 20 }}
        animate={{ scale: 1, y: 0 }}
        transition={{ type: "spring", stiffness: 260, damping: 26 }}
      >
        <div className="launch-cover">
          <Cover game={game} />
        </div>
        <div className="launch-text">
          <span className="qm-label">Avvio in corso</span>
          <span className="launch-name">{game.name}</span>
          <span className="launch-bar">
            <span className="launch-bar-fill" />
          </span>
          {waited >= SLOW_SECS && (
            <span className="launch-note">
              In attesa da {formatPlaytime(waited)}. Alcuni giochi, soprattutto Game Pass, ci mettono qualche
              minuto: appena compare la finestra del gioco te la porto davanti.
            </span>
          )}
          <span className="launch-hint">
            <span className="hint"><Glyph button="B" /> Nascondi</span>
            <span className="hint"><Glyph button="X" /> Annulla avvio</span>
          </span>
        </div>
      </motion.div>
    </motion.div>
  );
}
