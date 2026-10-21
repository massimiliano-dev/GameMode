import { motion } from "motion/react";
import { useEffect, useRef, useState } from "react";

import { Cover } from "../components/GameCard";
import { useClock, useHints, useScreenFocus } from "../components/common";
import { Priority, useAction } from "../input/dispatcher";
import { api } from "../lib/api";
import { formatClock, formatDate, formatPlaytime } from "../lib/format";
import type { PowerAction, Volume } from "../lib/types";
import { useApp } from "../store/app";

const VOLUME_STEP = 0.05;

export function QuickMenu() {
  const close = () => useApp.getState().setQuickMenu(false);
  const session = useApp((s) => s.session);
  const game = useApp((s) => s.games.find((g) => g.id === s.session?.gameId));
  const rescan = useApp((s) => s.rescan);
  const toast = useApp((s) => s.toast);
  const now = useClock(1000);
  const [volume, setVolume] = useState<Volume | null>(null);
  const [armed, setArmed] = useState<string | null>(null);
  const previousHints = useRef(useApp.getState().hints);

  useEffect(() => {
    void api.getVolume().then(setVolume).catch(() => setVolume(null));
    const hints = previousHints.current;
    return () => useApp.getState().setHints(hints);
  }, []);
  useScreenFocus();
  useHints([
    { button: "A", label: "Seleziona" },
    { button: "B", label: "Chiudi" },
  ]);

  // Modal: swallow everything but navigation so screens underneath stay still.
  useAction((action) => {
    if (action === "back" || action === "menu" || action === "guide") {
      close();
      return true;
    }
    return !["up", "down", "left", "right", "confirm"].includes(action);
  }, Priority.MODAL);

  /** Dangerous actions need a second press. */
  const confirmTwice = (id: string, run: () => void) => {
    if (armed === id) {
      setArmed(null);
      run();
    } else {
      setArmed(id);
    }
  };

  const power = (action: PowerAction) => () =>
    confirmTwice(action, () => {
      close();
      void api.power(action).catch((e) => toast(String(e), "error"));
    });

  const changeVolume = async (delta: number) => {
    if (!volume) return;
    const level = Math.min(1, Math.max(0, Math.round((volume.level + delta) * 20) / 20));
    setVolume({ ...volume, level });
    await api.setVolume(level);
  };

  const elapsed = session ? Math.max(0, Math.floor(Date.now() / 1000) - session.startedAt) : 0;

  return (
    <div className="quickmenu" data-nav-layer="quickmenu">
      <motion.div
        className="quickmenu-scrim"
        onClick={close}
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
      />
      <motion.aside
        className="quickmenu-panel"
        initial={{ x: "100%" }}
        animate={{ x: 0 }}
        exit={{ x: "100%" }}
        transition={{ type: "spring", stiffness: 380, damping: 38 }}
        onBlur={() => setArmed(null)}
      >
        <div className="qm-clock">
          <span className="qm-time">{formatClock(now)}</span>
          <span className="qm-date">{formatDate(now)}</span>
        </div>

        {session && game && (
          <div className="qm-game">
            <div className="qm-game-cover">
              <Cover game={game} />
            </div>
            <div className="qm-game-info">
              <span className="qm-label">{session.phase === "launching" ? "Avvio in corso" : "In gioco"}</span>
              <span className="qm-game-name">{game.name}</span>
              <span className="qm-sub">{formatPlaytime(elapsed)}</span>
            </div>
            <div className="qm-game-actions">
              <QmButton
                autofocus
                onClick={async () => {
                  close();
                  if (!(await api.resumeGame())) toast("Finestra del gioco non trovata");
                }}
              >
                Torna al gioco
              </QmButton>
              <QmButton danger armed={armed === "close"} onClick={() => confirmTwice("close", () => void api.closeGame())}>
                Chiudi gioco
              </QmButton>
            </div>
          </div>
        )}

        {volume && (
          <VolumeRow
            volume={volume}
            autofocus={!session}
            onAdjust={(dir) => void changeVolume(dir * VOLUME_STEP)}
            onToggleMute={async () => {
              const muted = !volume.muted;
              setVolume({ ...volume, muted });
              await api.setMuted(muted);
            }}
          />
        )}

        <div className="qm-list">
          <QmButton autofocus={!session && !volume} onClick={() => { close(); void rescan(); }}>
            Cerca di nuovo i giochi
          </QmButton>
          <QmButton onClick={() => void api.toggleFullscreen()}>Schermo intero sì/no</QmButton>
          <QmButton onClick={() => { close(); void api.exitToDesktop(); }}>Vai al desktop</QmButton>
        </div>

        <div className="qm-list qm-power">
          <QmButton armed={armed === "sleep"} onClick={power("sleep")}>Sospendi</QmButton>
          <QmButton armed={armed === "restart"} onClick={power("restart")}>Riavvia</QmButton>
          <QmButton danger armed={armed === "shutdown"} onClick={power("shutdown")}>Spegni</QmButton>
          <QmButton armed={armed === "quit"} onClick={() => confirmTwice("quit", () => void api.quit())}>
            Chiudi GameMode
          </QmButton>
        </div>
      </motion.aside>
    </div>
  );
}

function QmButton(props: {
  children: React.ReactNode;
  onClick: () => void;
  autofocus?: boolean;
  danger?: boolean;
  armed?: boolean;
}) {
  return (
    <button
      type="button"
      className={`qm-button ${props.danger ? "is-danger" : ""} ${props.armed ? "is-armed" : ""}`}
      data-nav
      data-nav-autofocus={props.autofocus || undefined}
      onClick={props.onClick}
    >
      {props.children}
      {props.armed && <span className="qm-confirm">Premi di nuovo per confermare</span>}
    </button>
  );
}

function VolumeRow(props: {
  volume: Volume;
  autofocus?: boolean;
  onAdjust: (dir: number) => void;
  onToggleMute: () => void;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const adjust = useRef(props.onAdjust);
  adjust.current = props.onAdjust;

  useEffect(() => {
    const el = ref.current;
    const listener = (e: Event) => adjust.current((e as CustomEvent<number>).detail);
    el?.addEventListener("navadjust", listener);
    return () => el?.removeEventListener("navadjust", listener);
  }, []);

  const pct = Math.round(props.volume.level * 100);
  return (
    <button
      ref={ref}
      type="button"
      className={`qm-volume ${props.volume.muted ? "is-muted" : ""}`}
      data-nav
      data-nav-capture="x"
      data-nav-autofocus={props.autofocus || undefined}
      onClick={props.onToggleMute}
    >
      <span className="qm-label">
        Volume <span className="qm-sub">{props.volume.muted ? "muto · A per riattivare" : "◀ ▶ per regolare · A muto"}</span>
      </span>
      <span className="qm-volume-bar">
        <span className="qm-volume-fill" style={{ width: `${props.volume.muted ? 0 : pct}%` }} />
      </span>
      <span className="qm-volume-value">{props.volume.muted ? "Muto" : `${pct}%`}</span>
    </button>
  );
}
