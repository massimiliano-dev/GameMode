import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { Priority, useAction } from "../input/dispatcher";
import { ensureFocus, focusElement } from "../input/focus";
import { api, artUrl, on } from "../lib/api";
import { formatLastPlayed } from "../lib/format";
import type { Capture, Game } from "../lib/types";
import { useApp } from "../store/app";
import { ConfirmDialog } from "./ConfirmDialog";
import { Glyph } from "./common";

/** How many shots the Home row shows, across every game. */
const HOME_LIMIT = 14;

/** Screenshots of one game (or all), kept fresh while new ones are taken. */
export function useCaptures(gameId?: string) {
  const [captures, setCaptures] = useState<Capture[] | null>(null);
  const reload = useCallback(() => {
    void api
      .listCaptures(gameId)
      .then(setCaptures)
      .catch(() => setCaptures([]));
  }, [gameId]);
  useEffect(() => {
    reload();
    return on<Capture>("capture://saved", reload);
  }, [reload]);
  const remove = useCallback(
    (path: string) => setCaptures((list) => list?.filter((c) => c.path !== path) ?? null),
    [],
  );
  return { captures, reload, remove };
}

function formatTaken(ts: number) {
  const d = new Date(ts * 1000);
  const time = d.toLocaleTimeString("it-IT", { hour: "2-digit", minute: "2-digit" });
  return `${formatLastPlayed(ts)} · ${time}`;
}

/** "Catture" section of a game's page: a grid of thumbnails, A opens the viewer. */
export function CaptureGallery({ game }: { game: Game }) {
  const { captures, remove } = useCaptures(game.id);
  const [viewing, setViewing] = useState<number | null>(null);

  if (!captures?.length) return null;

  return (
    <section className="detail-section">
      <h2 className="row-title detail-section-title">
        Catture
        <span className="row-count">{captures.length}</span>
      </h2>
      <div className="captures-grid" data-nav-group="captures">
        {captures.map((c, i) => (
          <button
            key={c.path}
            type="button"
            className="capture-thumb"
            data-nav
            data-nav-id={`capture:${c.path}`}
            aria-label={`Cattura del ${formatTaken(c.takenAt)}`}
            onClick={() => setViewing(i)}
          >
            <img src={artUrl(c.path)} alt="" loading="lazy" decoding="async" draggable={false} />
            <span className="capture-thumb-date">{formatTaken(c.takenAt)}</span>
          </button>
        ))}
      </div>
      {viewing !== null && <CaptureViewer captures={captures} index={viewing} onClose={() => setViewing(null)} onDeleted={remove} />}
    </section>
  );
}

/** Home row with the latest screenshots across all games. `onFocus` tells
 * Home the focus is here, so its game actions (X, Y) stand down. */
export function CaptureRow({ onFocus }: { onFocus?: () => void }) {
  const { captures, remove } = useCaptures();
  const setBackdrop = useApp((s) => s.setBackdrop);
  const [viewing, setViewing] = useState<number | null>(null);
  const recent = captures?.slice(0, HOME_LIMIT) ?? [];

  if (!recent.length) return null;

  return (
    <section className="row" data-nav-group="row:captures">
      <h2 className="row-title">
        Le tue catture
        <span className="row-count">{captures?.length}</span>
      </h2>
      <div className="row-scroll">
        {recent.map((c, i) => (
          <button
            key={c.path}
            type="button"
            className="capture-card"
            data-nav
            data-nav-id={`captures:${c.path}`}
            aria-label={`${c.gameName}, ${formatTaken(c.takenAt)}`}
            onFocus={() => {
              setBackdrop(c.gameId);
              onFocus?.();
            }}
            onClick={() => setViewing(i)}
          >
            <img src={artUrl(c.path)} alt="" loading="lazy" decoding="async" draggable={false} />
            <span className="capture-card-meta">
              <span className="capture-card-name">{c.gameName}</span>
              <span className="capture-card-date">{formatLastPlayed(c.takenAt)}</span>
            </span>
          </button>
        ))}
      </div>
      {viewing !== null && <CaptureViewer captures={recent} index={viewing} onClose={() => setViewing(null)} onDeleted={remove} />}
    </section>
  );
}

interface ViewerProps {
  captures: Capture[];
  /** Where to start; the viewer browses from there. */
  index: number;
  onClose: () => void;
  /** The shot at `path` was deleted: drop it from the list. */
  onDeleted: (path: string) => void;
}

/** Full-screen picture: ← → browse, X deletes, Y opens the folder, B closes. */
export function CaptureViewer({ captures, index: start, onClose: close, onDeleted }: ViewerProps) {
  const toast = useApp((s) => s.toast);
  const [confirm, setConfirm] = useState(false);
  const [i, setI] = useState(start);
  const count = captures.length;
  const index = Math.min(i, count - 1);
  const current = captures[index];
  // The thumbnail that opened the viewer. The delete dialog moves the focus
  // away, so it has to be remembered rather than left in place.
  const opener = useRef(document.activeElement as HTMLElement | null);

  // Back on that thumbnail; if it was deleted, on its row or grid.
  const onClose = () => {
    const el = opener.current;
    const group = el?.closest<HTMLElement>("[data-nav-group]");
    close();
    requestAnimationFrame(() => {
      const target = el?.isConnected ? el : group?.isConnected ? group.querySelector<HTMLElement>("[data-nav]") : null;
      if (target) focusElement(target);
      else ensureFocus();
    });
  };

  const step = (dir: number) => {
    if (count > 1) setI((index + dir + count) % count);
  };

  const remove = async () => {
    setConfirm(false);
    try {
      await api.deleteCapture(current.path);
      toast("Cattura eliminata");
      if (count <= 1) onClose();
      else setI(Math.min(index, count - 2));
      onDeleted(current.path);
    } catch (e) {
      toast(String(e), "error");
    }
  };

  useAction((action, repeat) => {
    if (confirm) return;
    switch (action) {
      case "left":
      case "lb":
        step(-1);
        return true;
      case "right":
      case "rb":
        step(1);
        return true;
      case "back":
      case "confirm":
        if (!repeat) onClose();
        return true;
      case "x":
        if (!repeat) setConfirm(true);
        return true;
      case "y":
        if (!repeat) void api.openCapturesFolder(current.gameId).catch((e) => toast(String(e), "error"));
        return true;
    }
    return true;
  }, Priority.BLOCKING);

  if (!current) return null;

  // Outside the scrolling rows (their fade mask would clip it) but inside
  // the app, so the toasts still stack above it.
  return createPortal(
    <motion.div
      className="capture-viewer"
      data-nav-layer="viewer"
      role="dialog"
      aria-label={current.gameName}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ duration: 0.15 }}
    >
      <motion.img
        key={current.path}
        className="capture-viewer-image"
        src={artUrl(current.path)}
        alt=""
        draggable={false}
        initial={{ opacity: 0, scale: 0.98 }}
        animate={{ opacity: 1, scale: 1 }}
        transition={{ duration: 0.18 }}
      />
      <div className="capture-viewer-bar">
        <span className="capture-viewer-info">
          <span className="capture-viewer-name">{current.gameName}</span>
          <span className="qm-sub">
            {formatTaken(current.takenAt)}
            {count > 1 && ` · ${index + 1} di ${count}`}
          </span>
        </span>
        <span className="qm-hints">
          {count > 1 && (
            <span className="hint">
              <Glyph button="LB" />
              <Glyph button="RB" />
              Sfoglia
            </span>
          )}
          <span className="hint">
            <Glyph button="Y" />
            Apri cartella
          </span>
          <span className="hint">
            <Glyph button="X" />
            Elimina
          </span>
          <span className="hint">
            <Glyph button="B" />
            Chiudi
          </span>
        </span>
      </div>
      <AnimatePresence>
        {confirm && (
          <ConfirmDialog
            key="delete"
            title="Eliminare questa cattura?"
            text="Il file sparisce dalla cartella Immagini. Non si può annullare."
            confirmLabel="Elimina"
            onCancel={() => setConfirm(false)}
            onConfirm={() => void remove()}
          />
        )}
      </AnimatePresence>
    </motion.div>,
    document.querySelector(".app") ?? document.body,
  );
}
