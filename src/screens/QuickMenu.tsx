import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";

import { ActionSheet } from "../components/ActionSheet";
import { Cover } from "../components/GameCard";
import { Avatar } from "../components/ProfileSettings";
import { SLEEP_CHOICES, sleepLabel } from "../components/SystemSettings";
import { Glyph, useClock, useScreenFocus } from "../components/common";
import { Priority, useAction } from "../input/dispatcher";
import { api } from "../lib/api";
import { formatClock, formatDate, formatPlaytime } from "../lib/format";
import type { AudioOutput, PowerAction, Volume } from "../lib/types";
import { useApp } from "../store/app";

const VOLUME_STEP = 0.05;

export function QuickMenu() {
  const close = () => useApp.getState().setQuickMenu(false);
  const session = useApp((s) => s.session);
  const game = useApp((s) => s.games.find((g) => g.id === s.session?.gameId));
  const paused = useApp((s) => s.suspended.length);
  const rescan = useApp((s) => s.rescan);
  const profiles = useApp((s) => s.profiles);
  const activeProfile = useApp((s) => s.activeProfile);
  const switchProfile = useApp((s) => s.switchProfile);
  const toast = useApp((s) => s.toast);
  const now = useClock(1000);
  const [volume, setVolume] = useState<Volume | null>(null);
  const [armed, setArmed] = useState<string | null>(null);
  const [outputs, setOutputs] = useState<AudioOutput[] | null>(null);
  const output = outputs?.find((o) => o.default);
  const [consoleMode, setConsoleMode] = useState(false);
  const [sleepSheet, setSleepSheet] = useState(false);
  const settings = useApp((s) => s.settings);
  const saveSettings = useApp((s) => s.saveSettings);
  // The screen below keeps the hint bar; the panel shows its own.
  useEffect(() => {
    void api.getVolume().then(setVolume).catch(() => setVolume(null));
    void api.getConsoleMode().then((s) => setConsoleMode(s.active)).catch(() => {});
  }, []);
  useScreenFocus();

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
  const pausedNote = paused ? `${paused === 1 ? "Il gioco in pausa viene chiuso" : `I ${paused} giochi in pausa vengono chiusi`} · premi di nuovo` : undefined;
  const pausedQuitNote = paused ? `${paused === 1 ? "Il gioco in pausa riprende sul desktop" : "I giochi in pausa riprendono sul desktop"} · premi di nuovo` : undefined;

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

        {profiles.length > 1 && (
          <div className="qm-profiles" data-nav-group="qm-profiles">
            {profiles.map((p) => (
              <button
                key={p.id}
                type="button"
                className={`qm-profile ${p.id === activeProfile ? "is-active" : ""}`}
                data-nav
                onClick={() => void switchProfile(p.id)}
              >
                <Avatar profile={p} />
                <span>{p.name}</span>
              </button>
            ))}
          </div>
        )}

        {session && game && (
          <div className="qm-game">
            <div className="qm-game-cover">
              <Cover game={game} />
            </div>
            <div className="qm-game-info">
              <span className="qm-label">{session.update ? "Aggiornamento in corso" : session.phase === "launching" ? "Avvio in corso" : "In gioco"}</span>
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

        {paused > 0 && (
          <div className="qm-paused">
            <span className="qm-label">In pausa</span>
            <span className="qm-sub">{paused === 1 ? "Un gioco è in pausa: lo riprendi dalla Home" : `${paused} giochi in pausa: li riprendi dalla Home`}</span>
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
          <QmButton
            onClick={() =>
              void api
                .audioOutputs()
                .then(setOutputs)
                .catch((e) => toast(String(e), "error"))
            }
          >
            Uscita audio
          </QmButton>
          <QmButton
            onClick={() => {
              close();
              useApp.getState().open({ name: "connections" });
            }}
          >
            Wi-Fi e Bluetooth
          </QmButton>
          <QmButton autofocus={!session && !volume} onClick={() => { close(); void rescan(); }}>
            Cerca di nuovo i giochi
          </QmButton>
          <QmButton onClick={() => void api.toggleFullscreen()}>Schermo intero sì/no</QmButton>
          <QmButton
            armed={armed === "desktop" && paused > 0}
            confirmText={pausedQuitNote}
            onClick={() => {
              if (paused > 0 && armed !== "desktop") return setArmed("desktop");
              setArmed(null);
              close();
              void useApp.getState().exitToDesktop();
            }}
          >
            {consoleMode ? "Apri il desktop di Windows" : "Vai al desktop"}
          </QmButton>
          {settings && (
            <QmButton onClick={() => setSleepSheet(true)}>
              Sospensione automatica <span className="qm-sub">{sleepLabel(settings.autoSleepMinutes)}</span>
            </QmButton>
          )}
        </div>

        <div className="qm-list qm-power">
          <QmButton armed={armed === "sleep"} onClick={power("sleep")}>Sospendi</QmButton>
          <QmButton armed={armed === "restart"} confirmText={pausedNote} onClick={power("restart")}>Riavvia</QmButton>
          <QmButton danger armed={armed === "shutdown"} confirmText={pausedNote} onClick={power("shutdown")}>Spegni</QmButton>
          <QmButton armed={armed === "quit"} confirmText={pausedQuitNote} onClick={() => confirmTwice("quit", () => void useApp.getState().quit())}>
            {consoleMode ? "Chiudi GameMode e apri il desktop" : "Chiudi GameMode"}
          </QmButton>
        </div>
        <div className="qm-hints">
          <span className="hint"><Glyph button="A" /> Seleziona</span>
          <span className="hint"><Glyph button="B" /> Chiudi</span>
        </div>
      </motion.aside>
      <AnimatePresence>
        {outputs && (
          <ActionSheet
            key="audio"
            title={output ? `Uscita audio · ${output.name}` : "Uscita audio"}
            onClose={() => setOutputs(null)}
            actions={outputs.map((o) => ({
              label: o.name,
              hint: o.default ? "In uso" : undefined,
              onSelect: () =>
                void api
                  .setAudioOutput(o.id)
                  .then(() => toast(`Audio su ${o.name}`))
                  .catch((e) => toast(String(e), "error")),
            }))}
          />
        )}
        {sleepSheet && settings && (
          <ActionSheet
            key="sleep"
            title="Sospensione automatica"
            onClose={() => setSleepSheet(false)}
            actions={SLEEP_CHOICES.map(({ minutes, label }) => ({
              label,
              hint: minutes === settings.autoSleepMinutes ? "Scelta attuale" : undefined,
              onSelect: () => {
                saveSettings({ ...settings, autoSleepMinutes: minutes })
                  .then(() =>
                    toast(minutes ? `Il PC si sospende dopo ${label.replace("Dopo ", "")} senza attività` : "Sospensione automatica disattivata"),
                  )
                  .catch((e) => toast(String(e), "error"));
              },
            }))}
          />
        )}
      </AnimatePresence>
    </div>
  );
}

export function QmButton(props: {
  children: React.ReactNode;
  onClick: () => void;
  autofocus?: boolean;
  danger?: boolean;
  armed?: boolean;
  /** What the second press means, when it isn't a plain confirmation. */
  confirmText?: string;
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
      {props.armed && <span className="qm-confirm">{props.confirmText ?? "Premi di nuovo per confermare"}</span>}
    </button>
  );
}

export function VolumeRow(props: {
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
