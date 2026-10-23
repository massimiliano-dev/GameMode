import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { api } from "../lib/api";
import { formatClock, formatDate } from "../lib/format";
import type { Game, Session } from "../lib/types";
import { useApp } from "../store/app";
import { useIdle } from "../store/idle";
import { Cover } from "./GameCard";
import { useClock } from "./common";

/** How long each cover stays before the next fades in. */
const COVER_EVERY = 25_000;
/** The clock drifts a little now and then, against burn-in. */
const DRIFT_EVERY = 60_000;
const MAX_COVERS = 8;

/**
 * Ambient screen after a while without input: a clock and the covers of the
 * most played games. Any button, key or mouse move brings GameMode back.
 * With the display off it is plain black, so nothing flashes when it wakes.
 */
export function Screensaver() {
  const stage = useIdle((s) => s.stage);
  // The last-minute power warning must stay visible and get the input.
  const countdown = useIdle((s) => s.countdown);
  return <AnimatePresence>{stage !== "active" && !countdown && <Ambient key="ambient" dark={stage === "displayOff"} />}</AnimatePresence>;
}

function Ambient({ dark }: { dark: boolean }) {
  const wake = useIdle((s) => s.wake);
  const games = useApp((s) => s.games);
  const now = useClock(1000);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [index, setIndex] = useState(0);
  const [drift, setDrift] = useState({ x: 0, y: 0 });

  useAction(() => {
    wake();
    return true;
  }, Priority.BLOCKING);

  useEffect(() => {
    const onMouse = () => wake();
    window.addEventListener("mousemove", onMouse, { passive: true });
    window.addEventListener("mousedown", onMouse);
    return () => {
      window.removeEventListener("mousemove", onMouse);
      window.removeEventListener("mousedown", onMouse);
    };
  }, [wake]);

  useEffect(() => {
    void api.getSessions().then(setSessions).catch(() => setSessions([]));
  }, []);

  const covers = useMemo(() => mostPlayed(games, sessions), [games, sessions]);

  useEffect(() => {
    if (covers.length < 2) return;
    const t = setInterval(() => setIndex((i) => (i + 1) % covers.length), COVER_EVERY);
    return () => clearInterval(t);
  }, [covers.length]);

  useEffect(() => {
    const t = setInterval(() => setDrift({ x: Math.round(Math.random() * 6 - 3), y: Math.round(Math.random() * 6 - 3) }), DRIFT_EVERY);
    return () => clearInterval(t);
  }, []);

  const game = covers[index % Math.max(1, covers.length)];

  return (
    <motion.div
      className={`screensaver ${dark ? "is-dark" : ""}`}
      data-nav-layer="screensaver"
      role="presentation"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 1.2 }}
    >
      {!dark && (
        <div className="screensaver-inner" style={{ transform: `translate(${drift.x}vw, ${drift.y}vh)` }}>
          <AnimatePresence mode="sync">
            {game && (
              <motion.div
                key={game.id}
                className="screensaver-cover"
                initial={{ opacity: 0, scale: 0.98 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 1.6, ease: "easeInOut" }}
              >
                <Cover game={game} />
              </motion.div>
            )}
          </AnimatePresence>
          <div className="screensaver-clock">
            <span className="screensaver-time">{formatClock(now)}</span>
            <span className="screensaver-date">{formatDate(now)}</span>
          </div>
        </div>
      )}
    </motion.div>
  );
}

/** Games with a cover, most played first; recently played ones fill in. */
export function mostPlayed(games: Game[], sessions: Session[]): Game[] {
  const byGame = new Map<string, number>();
  for (const s of sessions) byGame.set(s.gameId, (byGame.get(s.gameId) ?? 0) + s.seconds);
  const score = (g: Game) => (byGame.get(g.id) ?? 0) + g.playtimeSecs;
  return games
    .filter((g) => !g.hidden && g.artwork.cover)
    .sort((a, b) => score(b) - score(a) || (b.lastPlayed ?? 0) - (a.lastPlayed ?? 0))
    .slice(0, MAX_COVERS);
}
