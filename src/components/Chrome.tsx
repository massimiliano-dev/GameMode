import { AnimatePresence, motion } from "motion/react";

import { formatClock } from "../lib/format";
import { activeProfileOf, type Tab, useApp } from "../store/app";
import { Logo } from "./Logo";
import { Avatar } from "./ProfileSettings";
import { StatusCluster } from "./StatusBar";
import { Glyph, useClock } from "./common";

const TABS: { tab: Tab; label: string }[] = [
  { tab: "home", label: "Home" },
  { tab: "library", label: "Libreria" },
  { tab: "downloads", label: "Scarica" },
  { tab: "stores", label: "Negozi" },
  { tab: "stats", label: "Statistiche" },
  { tab: "settings", label: "Impostazioni" },
];

export function TopBar() {
  const now = useClock();
  const active = useApp((s) => s.stack[0].screen.name);
  const setTab = useApp((s) => s.setTab);
  const session = useApp((s) => s.session);
  const playing = useApp((s) => s.games.find((g) => g.id === s.session?.gameId));
  const paused = useApp((s) => s.suspended.length);

  return (
    <header className="topbar">
      <div className="brand">
        <Logo size={30} />
        GameMode
      </div>
      <nav className="tabs" data-nav-group="tabs">
        <button
          type="button"
          data-nav
          data-nav-id="tab:search"
          className="tab tab-search"
          aria-label="Cerca"
          onClick={() => useApp.getState().open({ name: "search" })}
        >
          ⌕
        </button>
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
        <ProfileBadge />
        <Batteries />
        <StatusCluster />
        {session && playing && (
          <span className="session-pill">
            <span className="pulse" />
            {session.update ? "Aggiornamento" : session.phase === "launching" ? "Avvio" : "In gioco"}: {playing.name}
          </span>
        )}
        {paused > 0 && (
          <span className="session-pill is-paused">
            {paused === 1 ? "1 gioco in pausa" : `${paused} giochi in pausa`}
          </span>
        )}
        <span className="clock">{formatClock(now)}</span>
      </div>
    </header>
  );
}

function ProfileBadge() {
  const profile = useApp(activeProfileOf);
  const many = useApp((s) => s.profiles.length > 1);
  if (!profile || !many) return null;
  return (
    <span className="profile-badge">
      <Avatar profile={profile} size="sm" />
      {profile.name}
    </span>
  );
}

function Batteries() {
  const controllers = useApp((s) => s.controllers);
  return (
    <>
      {controllers.map((c) => (
        <span
          key={c.slot}
          className={`battery ${c.wired ? "is-wired" : ""} ${!c.wired && c.level <= 1 ? "is-low" : ""}`}
          title={c.wired ? `Controller ${c.slot + 1}: cablato` : `Controller ${c.slot + 1}: batteria`}
        >
          <svg className="battery-pad" viewBox="0 0 24 16" aria-hidden>
            <path
              d="M6 1h12a6 6 0 0 1 5.6 8.2l-1.3 3.4A3 3 0 0 1 17 13.4L15.5 11h-7L7 13.4a3 3 0 0 1-5.3-.8L.4 9.2A6 6 0 0 1 6 1Zm1 3v2H5v2h2v2h2V8h2V6H9V4H7Zm9.5 1a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3Z"
              fill="currentColor"
            />
          </svg>
          {c.wired ? (
            <span className="battery-plug">⚡</span>
          ) : (
            <span className="battery-cells">
              {[0, 1, 2].map((i) => (
                <span key={i} className={`battery-cell ${c.level > i ? "is-full" : ""}`} />
              ))}
            </span>
          )}
        </span>
      ))}
    </>
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
        <Glyph button="view" />
        Cerca
      </span>
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
