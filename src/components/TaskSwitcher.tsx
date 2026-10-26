import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { ensureFocus, focusElement } from "../input/focus";
import { api, artUrl } from "../lib/api";
import type { OpenWindow } from "../lib/types";
import { useApp } from "../store/app";
import { Glyph } from "./common";

/** A window asked to close and still there after this long doesn't close by itself. */
const STUCK_MS = 4000;
/** How often the list follows windows opening and closing. */
const REFRESH_MS = 2000;
/** Swiping a card up this far (px) closes it, like on a phone. */
const SWIPE_CLOSE = 120;

const keyOf = (w: OpenWindow) => `${w.hwnd}:${w.gameId ?? ""}`;
const navId = (w: OpenWindow) => `sw:${keyOf(w)}`;

/**
 * Open windows as cards, like a phone's recent apps: A switches to one,
 * X closes it (again on one that doesn't close: ends it), Y closes them all,
 * B goes back. On a touch screen, a card swiped up closes.
 */
export function TaskSwitcher({ onClose, onGuide }: { onClose: () => void; onGuide?: () => void }) {
  const toast = useApp((s) => s.toast);
  const [windows, setWindows] = useState<OpenWindow[] | null>(null);
  /** When each card was asked to close, by key. */
  const [closing, setClosing] = useState<Record<string, number>>({});
  /** "all", or the key of a card whose next X ends its program. */
  const [armed, setArmed] = useState<string | null>(null);
  const focusedKey = useRef<string | null>(null);
  const focusedIndex = useRef(0);
  const mounted = useRef(true);

  const refresh = useCallback(async (thumbnails: boolean) => {
    const list = await api.openWindows(thumbnails).catch(() => null);
    if (!list || !mounted.current) return;
    // Quick refreshes carry no pictures: keep the ones already taken.
    setWindows((prev) =>
      list.map((w) => {
        if (w.thumbnail || !prev || w.hwnd === 0) return w;
        const old = prev.find((p) => p.hwnd === w.hwnd);
        return old?.thumbnail ? { ...w, thumbnail: old.thumbnail, thumbnailAt: old.thumbnailAt } : w;
      }),
    );
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh(false).then(() => refresh(true));
    const timer = setInterval(() => void refresh(false), REFRESH_MS);
    return () => {
      mounted.current = false;
      clearInterval(timer);
    };
  }, [refresh]);

  // Closed windows leave the list: forget them, and keep the focus where it was.
  useEffect(() => {
    if (!windows) return;
    const keys = new Set(windows.map(keyOf));
    setClosing((c) => {
      const kept = Object.entries(c).filter(([k]) => keys.has(k));
      return kept.length === Object.keys(c).length ? c : Object.fromEntries(kept);
    });
    if (focusedKey.current && keys.has(focusedKey.current)) return;
    const next = windows[Math.min(focusedIndex.current, windows.length - 1)];
    requestAnimationFrame(() => {
      const el = next ? document.querySelector<HTMLElement>(`[data-nav-id="${CSS.escape(navId(next))}"]`) : null;
      if (el) focusElement(el);
      else ensureFocus();
    });
  }, [windows]);

  const switchTo = async (w: OpenWindow) => {
    try {
      await api.switchToWindow(w.hwnd, w.gameId ?? null);
      onClose();
    } catch (e) {
      toast(String(e), "error");
      void refresh(false);
    }
  };

  const close = async (w: OpenWindow) => {
    const key = keyOf(w);
    const since = closing[key];
    const stuck = w.hung || (since !== undefined && Date.now() - since > STUCK_MS);
    // Already asked: give it time to save and go.
    if (since !== undefined && !stuck) return;
    try {
      if (stuck && w.canForce) {
        if (armed !== key) return setArmed(key);
        setArmed(null);
        await api.closeWindow(w.hwnd, w.gameId ?? null, true);
      } else {
        setClosing((c) => ({ ...c, [key]: Date.now() }));
        await api.closeWindow(w.hwnd, w.gameId ?? null, false);
      }
    } catch (e) {
      toast(String(e), "error");
      setClosing((c) => {
        const next = { ...c };
        delete next[key];
        return next;
      });
    }
    setTimeout(() => void refresh(false), 600);
  };

  const closeAll = async () => {
    if (!windows?.length) return;
    if (armed !== "all") return setArmed("all");
    setArmed(null);
    const at = Date.now();
    setClosing(Object.fromEntries(windows.map((w) => [keyOf(w), at])));
    const n = await api.closeAllWindows().catch((e) => {
      toast(String(e), "error");
      return 0;
    });
    if (n) toast(n === 1 ? "Chiudo un'app" : `Chiudo ${n} app`);
    setTimeout(() => void refresh(false), 600);
  };

  const focused = () => windows?.find((w) => keyOf(w) === focusedKey.current);

  // Modal: everything but moving and choosing stays here.
  useAction((action, repeat) => {
    if (action === "back") {
      if (!repeat) onClose();
      return true;
    }
    if (action === "guide" || action === "menu") {
      if (!repeat) (onGuide ?? onClose)();
      return true;
    }
    if (action === "x" || action === "y") {
      if (repeat) return true;
      if (action === "y") void closeAll();
      else {
        const w = focused();
        if (w) void close(w);
      }
      return true;
    }
    return !["up", "down", "left", "right", "confirm"].includes(action);
  }, Priority.BLOCKING);

  const count = windows?.length ?? 0;

  return (
    <motion.div
      className="switcher"
      data-nav-layer="switcher"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.18 }}
    >
      <header className="switcher-head">
        <h1 className="switcher-title">
          App aperte {count > 0 && <span className="row-count">{count}</span>}
        </h1>
        {armed === "all" && <span className="switcher-armed">Premi di nuovo Y per chiuderle tutte</span>}
      </header>

      {windows === null ? (
        <div className="switcher-empty">
          <div className="spinner" />
        </div>
      ) : count === 0 ? (
        <div className="switcher-empty">
          <p className="empty-title">Nessuna app aperta</p>
          <p className="empty-text">Qui compaiono i giochi e le app aperte, anche quelle avviate fuori da GameMode.</p>
          <button type="button" className="btn btn-primary" data-nav data-nav-autofocus onClick={onClose}>
            Indietro
          </button>
        </div>
      ) : (
        <div className="switcher-row" data-nav-group="switcher">
          <AnimatePresence initial={false}>
            {windows.map((w, i) => {
              const key = keyOf(w);
              return (
                <SwitcherCard
                  key={key}
                  window={w}
                  closingSince={closing[key]}
                  armed={armed === key}
                  onFocus={() => {
                    focusedKey.current = key;
                    focusedIndex.current = i;
                    if (armed && armed !== key) setArmed(null);
                  }}
                  onSelect={() => void switchTo(w)}
                  onClose={() => void close(w)}
                />
              );
            })}
          </AnimatePresence>
        </div>
      )}

      <footer className="switcher-hints">
        {count > 0 && (
          <>
            <span className="hint"><Glyph button="A" /> Passa all'app</span>
            <span className="hint"><Glyph button="X" /> Chiudi</span>
            <span className="hint"><Glyph button="Y" /> Chiudi tutte</span>
          </>
        )}
        <span className="hint"><Glyph button="B" /> Indietro</span>
      </footer>
    </motion.div>
  );
}

function SwitcherCard(props: {
  window: OpenWindow;
  closingSince?: number;
  armed: boolean;
  onFocus: () => void;
  onSelect: () => void;
  onClose: () => void;
}) {
  const { window: w, closingSince, armed } = props;
  const dragged = useRef(false);
  const stuck = w.hung || (closingSince !== undefined && Date.now() - closingSince > STUCK_MS);
  const status = armed
    ? "Premi di nuovo X per forzare la chiusura"
    : closingSince !== undefined && !stuck
      ? "Chiusura…"
      : stuck && closingSince !== undefined
        ? w.canForce
          ? "Non si chiude · X per forzarla"
          : "Non si chiude: guarda se chiede qualcosa"
        : w.hung
          ? "Non risponde"
          : null;
  const isApp = w.gameId?.startsWith("app:");

  return (
    <motion.div
      layout
      className={`sw-card ${closingSince !== undefined && !stuck ? "is-closing" : ""}`}
      role="button"
      tabIndex={-1}
      aria-label={w.name}
      data-nav
      data-nav-id={navId(w)}
      initial={{ opacity: 0, y: 24 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -160, transition: { duration: 0.22 } }}
      drag="y"
      dragConstraints={{ top: 0, bottom: 0 }}
      dragElastic={{ top: 0.8, bottom: 0.05 }}
      dragSnapToOrigin
      onDragStart={() => (dragged.current = true)}
      onDragEnd={(_, info) => {
        if (info.offset.y < -SWIPE_CLOSE || info.velocity.y < -900) props.onClose();
        setTimeout(() => (dragged.current = false), 0);
      }}
      onFocus={props.onFocus}
      onClick={() => {
        if (!dragged.current) props.onSelect();
      }}
    >
      <div className="sw-head">
        <SwitcherIcon window={w} />
        <span className="sw-names">
          <span className="sw-name">{w.name}</span>
          {w.detail && <span className="sw-detail">{w.detail}</span>}
        </span>
      </div>
      <div className="sw-preview">
        <SwitcherPicture window={w} />
        {w.state !== "open" && (
          <span className={`playing-pill sw-pill ${w.state === "paused" ? "is-paused" : ""}`}>
            {w.state === "paused" ? "In pausa" : isApp ? "In uso" : "In gioco"}
          </span>
        )}
        {status && <span className={`sw-status ${stuck || armed ? "is-stuck" : ""}`}>{status}</span>}
        <button
          type="button"
          className="sw-close"
          tabIndex={-1}
          aria-label={`Chiudi ${w.name}`}
          onClick={(e) => {
            e.stopPropagation();
            props.onClose();
          }}
        >
          ×
        </button>
      </div>
    </motion.div>
  );
}

/** The live picture of the window, else the game's art, else a big icon. */
function SwitcherPicture({ window: w }: { window: OpenWindow }) {
  const [failed, setFailed] = useState<string | null>(null);
  const thumb = artUrl(w.thumbnail, w.thumbnailAt);
  const art = artUrl(w.art, w.artVersion);
  const src = thumb && failed !== thumb ? thumb : art && failed !== art ? art : undefined;
  if (src) {
    return (
      <img
        className={src === thumb ? "sw-thumb" : "sw-art"}
        src={src}
        alt=""
        decoding="async"
        draggable={false}
        onError={() => setFailed(src)}
      />
    );
  }
  return (
    <span className="sw-big-icon">
      <SwitcherIcon window={w} />
    </span>
  );
}

function SwitcherIcon({ window: w }: { window: OpenWindow }) {
  const [failed, setFailed] = useState(false);
  const src = artUrl(w.icon);
  if (!src || failed) {
    const first = w.name.trim().match(/\p{L}|\p{N}/u)?.[0]?.toUpperCase() ?? "·";
    return (
      <span className="sw-icon sw-icon-placeholder" aria-hidden>
        {first}
      </span>
    );
  }
  return <img className="sw-icon" src={src} alt="" draggable={false} onError={() => setFailed(true)} />;
}
