import { motion } from "motion/react";
import { useEffect, useState } from "react";

import { CheatsPanel } from "../components/CheatsPanel";
import { Toasts } from "../components/Chrome";
import { PowerCountdown } from "../components/PowerCountdown";
import { Cover } from "../components/GameCard";
import { Glyph, useClock } from "../components/common";
import { MusicPanel } from "../components/NowPlaying";
import { Priority, useAction } from "../input/dispatcher";
import { focusInitial } from "../input/focus";
import { setupInput } from "../input/sources";
import { api, on } from "../lib/api";
import { formatClock, formatDate, formatPlaytime } from "../lib/format";
import type { Volume, VolumeEvent } from "../lib/types";
import { useApp } from "../store/app";
import { useIdle } from "../store/idle";
import { QmButton, VolumeRow } from "./QuickMenu";

const VOLUME_STEP = 0.05;

/** Runs in the transparent overlay window that Guide opens over a game. */
export function OverlayApp() {
  const session = useApp((s) => s.session);
  const game = useApp((s) => s.games.find((g) => g.id === s.session?.gameId));
  const controllers = useApp((s) => s.controllers);
  const toast = useApp((s) => s.toast);
  const now = useClock(1000);
  const [volume, setVolume] = useState<Volume | null>(null);
  const [armed, setArmed] = useState(false);
  const [suspendArmed, setSuspendArmed] = useState(false);
  const [suspending, setSuspending] = useState(false);
  const [openedAt, setOpenedAt] = useState(0);
  const [wand, setWand] = useState(false);
  const [view, setView] = useState<"menu" | "cheats">("menu");
  const show = (next: "menu" | "cheats") => {
    setView(next);
    requestAnimationFrame(() => focusInitial());
  };
  // The window stays mounted when hidden: the mixer polls only while it shows.
  const [open, setOpen] = useState(false);

  useEffect(() => {
    void useApp.getState().init();
    useIdle.getState().init();
    void setupInput();
    const refresh = () => {
      void api.getVolume().then(setVolume).catch(() => setVolume(null));
      void api.getSession().then((s) => useApp.setState({ session: s }));
      void api.wandInstalled().then(setWand).catch(() => setWand(false));
      // Settings are saved in the main window: re-read them at every opening.
      void api.getSettings().then((settings) => useApp.setState({ settings })).catch(() => {});
      setArmed(false);
      setSuspendArmed(false);
      setSuspending(false);
      setView("menu");
      setOpenedAt(Date.now());
      requestAnimationFrame(() => focusInitial());
    };
    refresh();
    const unlisten = on("overlay://open", () => {
      setOpen(true);
      refresh();
    });
    const unlistenClose = on("overlay://close", () => setOpen(false));
    const offVolume = on<VolumeEvent>("audio://volume", ({ level, muted }) => setVolume({ level, muted }));
    return () => {
      unlisten();
      unlistenClose();
      offVolume();
    };
  }, []);

  const resume = () => void api.overlayHide(true);
  const suspend = () => {
    if (suspending) return;
    // Online games kick a frozen player: ask first when an anti-cheat is around.
    if (session?.antiCheat && !suspendArmed) return setSuspendArmed(true);
    setSuspending(true);
    // On success the backend hides the overlay itself; on failure it stays
    // up, so the toast saying why can be read.
    void useApp.getState().suspendGame().then((ok) => {
      setSuspending(false);
      if (!ok) setSuspendArmed(false);
    });
  };

  useAction((action, repeat) => {
    if (repeat) return true;
    if (action === "back" || action === "guide" || action === "menu") {
      resume();
      return true;
    }
    return !["up", "down", "left", "right", "confirm"].includes(action);
  }, Priority.MODAL);

  const elapsed = session ? Math.max(0, Math.floor(Date.now() / 1000) - session.startedAt) : 0;

  return (
    <div className="overlay-root" data-nav-layer="overlay">
      <motion.aside
        key={openedAt}
        className="quickmenu-panel overlay-panel"
        initial={{ x: "100%" }}
        animate={{ x: 0 }}
        transition={{ type: "spring", stiffness: 380, damping: 38 }}
      >
        <div className="qm-clock">
          <span className="qm-time">{formatClock(now)}</span>
          <span className="qm-date">{formatDate(now)}</span>
        </div>

        {game && (
          <div className="qm-game">
            <div className="qm-game-cover">
              <Cover game={game} />
            </div>
            <div className="qm-game-info">
              <span className="qm-label">In gioco</span>
              <span className="qm-game-name">{game.name}</span>
              <span className="qm-sub">{formatPlaytime(elapsed)}</span>
            </div>
          </div>
        )}

        {view === "cheats" ? (
          <CheatsPanel onClose={() => show("menu")} />
        ) : (
          <>
          {controllers.length > 0 && (
            <div className="overlay-pads">
              {controllers.map((c) => (
                <span key={c.slot} className="qm-sub">
                  Controller {c.slot + 1}: {c.wired ? "cablato" : ["scarico", "batteria bassa", "batteria media", "batteria piena"][c.level]}
                </span>
              ))}
            </div>
          )}

          {volume && (
            <VolumeRow
              volume={volume}
              onAdjust={(dir) => {
                const level = Math.min(1, Math.max(0, Math.round((volume.level + dir * VOLUME_STEP) * 20) / 20));
                setVolume({ ...volume, level });
                void api.setVolume(level);
              }}
              onToggleMute={() => {
                setVolume({ ...volume, muted: !volume.muted });
                void api.setMuted(!volume.muted);
              }}
            />
          )}

          <MusicPanel active={open} showOpen />

          <div className="qm-list">
            <QmButton autofocus onClick={resume}>
              Torna al gioco
            </QmButton>
            {wand && game && (
              <>
                <QmButton onClick={() => void api.openWandApp()}>App Wand: scegli le mod e premi Play</QmButton>
                <QmButton onClick={() => void api.openWand()}>Overlay di Wand</QmButton>
              </>
            )}
            {session?.phase === "running" && <QmButton onClick={() => show("cheats")}>Trucchi</QmButton>}
            <QmButton onClick={() => void api.overlayKeyboard()}>Tastiera</QmButton>
            <QmButton onClick={() => void api.overlayToLauncher()}>Vai a GameMode</QmButton>
            {session?.phase === "running" && (
              <QmButton armed={suspendArmed} confirmText="Non consigliato per i giochi online · premi di nuovo" onClick={suspend}>
                Sospendi partita
              </QmButton>
            )}
            <QmButton
              danger
              armed={armed}
              onClick={() => {
                if (!armed) return setArmed(true);
                void api.closeGame().then((closed) => {
                  if (!closed) toast("Nessun gioco da chiudere");
                  void api.overlayHide(false);
                });
              }}
            >
              Chiudi gioco
            </QmButton>
          </div>

          <div className="qm-hints">
            <span className="hint"><Glyph button="A" /> Seleziona</span>
            <span className="hint"><Glyph button="B" /> Torna al gioco</span>
            <span className="hint"><Glyph button="view" /><Glyph button="menu" /> Apri/chiudi</span>
          </div>
          </>
        )}
      </motion.aside>
      <Toasts />
      <PowerCountdown />
    </div>
  );
}
