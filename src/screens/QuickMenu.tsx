import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, useState } from "react";

import { ActionSheet } from "../components/ActionSheet";
import { Cover } from "../components/GameCard";
import { Avatar } from "../components/ProfileSettings";
import { Glyph, useClock, useScreenFocus } from "../components/common";
import { MusicPanel } from "../components/NowPlaying";
import { Priority, useAction } from "../input/dispatcher";
import { api, on } from "../lib/api";
import { formatClock, formatDate, formatPlaytime } from "../lib/format";
import type { AudioOutput, PowerAction, PowerMode, PowerOptions, Volume, VolumeEvent } from "../lib/types";
import { useApp } from "../store/app";
import { useIdle } from "../store/idle";

const VOLUME_STEP = 0.05;
const POWER_VERB: Record<PowerAction, string> = { shutdown: "Spegnimento", restart: "Riavvio", sleep: "Sospensione", restartToDesktop: "Riavvio in modalità desktop", hibernate: "Ibernazione", lock: "Blocco", logoff: "Disconnessione" };

export function QuickMenu() {
  const close = () => useApp.getState().setQuickMenu(false);
  const session = useApp((s) => s.session);
  const game = useApp((s) => s.games.find((g) => g.id === s.session?.gameId));
  const isApp = game?.store === "app";
  const paused = useApp((s) => s.suspended.length);
  const backupOnExit = useApp((s) => s.settings?.backupOnExit ?? true);
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
  const [desktopOnce, setDesktopOnce] = useState(false);
  const schedule = useIdle((s) => s.schedule);
  const cancelPower = useIdle((s) => s.cancelPower);
  const [powerSheet, setPowerSheet] = useState(false);
  const [powerOptions, setPowerOptions] = useState<PowerOptions>({ sleep: true, hibernate: false });
  // The screen below keeps the hint bar; the panel shows its own.
  useEffect(() => {
    void api.getVolume().then(setVolume).catch(() => setVolume(null));
    void api.getConsoleMode().then((s) => { setConsoleMode(s.active); setDesktopOnce(s.desktopOnce); }).catch(() => {});
    void api.powerOptions().then(setPowerOptions).catch(() => {});
    // Volume keys and the other window keep the bar current.
    return on<VolumeEvent>("audio://volume", ({ level, muted }) => setVolume({ level, muted }));
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

  /** "Shut down in…": the backend warns a minute before. */
  const schedulePower = (action: PowerAction, mode: PowerMode) => () =>
    void api
      .schedulePower(action, mode)
      .then((s) => toast(s.at ? `${POWER_VERB[action]} alle ${formatClock(new Date(s.at * 1000))}` : `${POWER_VERB[action]} a fine download`))
      .catch((e) => toast(String(e), "error"));

  const changeVolume = async (delta: number) => {
    if (!volume) return;
    const level = Math.min(1, Math.max(0, Math.round((volume.level + delta) * 20) / 20));
    setVolume({ ...volume, level });
    await api.setVolume(level);
  };

  const elapsed = session ? Math.max(0, Math.floor(Date.now() / 1000) - session.startedAt) : 0;
  // Ending the session closes every open game gently (they get to save
  // first), the paused ones included, then backs the saves up if asked.
  const open = paused + (session ? 1 : 0);
  const saved = backupOnExit ? " e i salvataggi copiati" : "";
  const gameNote = open
    ? `${open === 1 ? "Il gioco viene chiuso" : `I ${open} giochi vengono chiusi`}${saved} · premi di nuovo`
    : undefined;
  // A scheduled shutdown closes the games too, without a second press.
  const scheduledNote = open ? `${open === 1 ? "Il gioco verrà chiuso" : `I ${open} giochi verranno chiusi`}${saved}` : undefined;
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
              <span className="qm-label">{session.update ? "Aggiornamento in corso" : session.phase === "launching" ? "Avvio in corso" : isApp ? "In uso" : "In gioco"}</span>
              <span className="qm-game-name">{game.name}</span>
              <span className="qm-sub">{formatPlaytime(elapsed)}</span>
            </div>
            <div className="qm-game-actions">
              <QmButton
                autofocus
                onClick={async () => {
                  close();
                  if (!(await api.resumeGame())) toast(isApp ? "Finestra dell'app non trovata" : "Finestra del gioco non trovata");
                }}
              >
                {isApp ? "Torna all'app" : "Torna al gioco"}
              </QmButton>
              <QmButton danger armed={armed === "close"} onClick={() => confirmTwice("close", () => void api.closeGame())}>
                {isApp ? "Chiudi app" : "Chiudi gioco"}
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

        <MusicPanel active showOpen onOpened={close} />

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
        </div>

        {schedule && (
          <div className="qm-paused qm-scheduled">
            <span className="qm-label">Programmato</span>
            <span className="qm-sub">
              {schedule.at ? `${POWER_VERB[schedule.action]} alle ${formatClock(new Date(schedule.at * 1000))}` : `${POWER_VERB[schedule.action]} a fine download`}
              {schedule.action !== "sleep" && scheduledNote ? ` · ${scheduledNote}` : ""}
            </span>
            <QmButton onClick={() => void cancelPower().then(() => toast("Programmazione annullata"))}>Annulla</QmButton>
          </div>
        )}

        <div className="qm-list qm-power">
          {powerOptions.sleep && <QmButton armed={armed === "sleep"} onClick={power("sleep")}>Sospendi</QmButton>}
          {powerOptions.hibernate && <QmButton armed={armed === "hibernate"} onClick={power("hibernate")}>Iberna</QmButton>}
          <QmButton
            onClick={() => {
              close();
              void api.restMode().catch((e) => toast(String(e), "error"));
            }}
          >
            Modalità riposo
            <span className="qm-sub">Schermo spento, i download continuano</span>
          </QmButton>
          <QmButton onClick={() => setPowerSheet(true)}>Spegni fra…</QmButton>
          {/* Unlocking needs a keyboard: not offered when GameMode is the shell. */}
          {!consoleMode && <QmButton armed={armed === "lock"} onClick={power("lock")}>Blocca</QmButton>}
          <QmButton armed={armed === "logoff"} confirmText={gameNote} onClick={power("logoff")}>Disconnetti</QmButton>
          <QmButton armed={armed === "restart"} confirmText={gameNote} onClick={power("restart")}>
            {desktopOnce ? "Riavvia in modalità console" : "Riavvia"}
          </QmButton>
          {consoleMode && (
            <QmButton armed={armed === "restartToDesktop"} confirmText={gameNote} onClick={power("restartToDesktop")}>
              Riavvia in modalità desktop
            </QmButton>
          )}
          <QmButton danger armed={armed === "shutdown"} confirmText={gameNote} onClick={power("shutdown")}>Spegni</QmButton>
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
        {powerSheet && (
          <ActionSheet
            key="power"
            title="Spegni fra"
            onClose={() => setPowerSheet(false)}
            actions={[
              { label: "30 minuti", hint: scheduledNote, onSelect: schedulePower("shutdown", { kind: "in", secs: 30 * 60 }) },
              { label: "1 ora", hint: scheduledNote, onSelect: schedulePower("shutdown", { kind: "in", secs: 60 * 60 }) },
              { label: "2 ore", hint: scheduledNote, onSelect: schedulePower("shutdown", { kind: "in", secs: 2 * 60 * 60 }) },
              {
                label: "A fine download",
                hint: scheduledNote ? `Quando non scarica più nulla da un minuto · ${scheduledNote}` : "Quando non scarica più nulla da un minuto",
                onSelect: schedulePower("shutdown", { kind: "atDownloadsEnd" }),
              },
              { label: "Sospendi a fine download", hint: "Il PC si sospende invece di spegnersi", onSelect: schedulePower("sleep", { kind: "atDownloadsEnd" }) },
            ]}
          />
        )}
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
