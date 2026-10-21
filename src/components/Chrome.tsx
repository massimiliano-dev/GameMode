import { AnimatePresence, motion } from "motion/react";

import { formatClock } from "../lib/format";
import { type Tab, useApp } from "../store/app";
import { Glyph, useClock } from "./common";

const TABS: { tab: Tab; label: string }[] = [
  { tab: "home", label: "Home" },
  { tab: "library", label: "Libreria" },
  { tab: "stores", label: "Negozi" },
  { tab: "settings", label: "Impostazioni" },
];

export function TopBar() {
  const now = useClock();
  const active = useApp((s) => s.stack[0].screen.name);
  const setTab = useApp((s) => s.setTab);
  const session = useApp((s) => s.session);
  const playing = useApp((s) => s.games.find((g) => g.id === s.session?.gameId));

  return (
    <header className="topbar">
      <div className="brand">
        <span className="brand-mark" />
        GameMode
      </div>
      <nav className="tabs" data-nav-group="tabs">
        <Glyph button="LB" />
        {TABS.map(({ tab, label }) => (
          <button
            key={tab}
            type="button"
            data-nav
            data-nav-id={`tab:${tab}`}
            className={`tab ${active === tab ? "is-active" : ""}`}
            onClick={() => setTab(tab)}
          >
            {label}
          </button>
        ))}
        <Glyph button="RB" />
      </nav>
      <div className="topbar-right">
        {session && playing && (
          <span className="session-pill">
            <span className="pulse" />
            {session.phase === "launching" ? "Avvio" : "In gioco"}: {playing.name}
          </span>
        )}
        <span className="clock">{formatClock(now)}</span>
      </div>
    </header>
  );
}

export function HintBar() {
  const hints = useApp((s) => s.hints);
  return (
    <footer className="hintbar">
      {hints.map((h) => (
        <span key={h.button + h.label} className="hint">
          <Glyph button={h.button} />
          {h.alt && <Glyph button={h.alt} />}
          {h.label}
        </span>
      ))}
      <span className="hint">
        <Glyph button="menu" />
        Menu rapido
      </span>
    </footer>
  );
}

export function Toasts() {
  const toasts = useApp((s) => s.toasts);
  return (
    <div className="toasts" role="status" aria-live="polite">
      <AnimatePresence>
        {toasts.map((t) => (
          <motion.div
            key={t.id}
            className={`toast toast-${t.tone}`}
            layout
            initial={{ opacity: 0, y: 16 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 8 }}
            transition={{ duration: 0.22 }}
          >
            {t.text}
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
