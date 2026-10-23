import { useEffect, useMemo, useRef, useState } from "react";

import { useHints, useScreenFocus } from "../components/common";
import { api } from "../lib/api";
import { formatPlaytime } from "../lib/format";
import type { Game, Session } from "../lib/types";
import { activeProfileOf, useApp } from "../store/app";

const DAYS = 14;
const DAY_MS = 86_400_000;

interface Day {
  start: number;
  secs: number;
  /** Most played game that day. */
  topGame: string | null;
}

function startOfDay(ms: number) {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** Seconds played per local day, splitting sessions that cross midnight. */
function perDay(sessions: Session[]) {
  const totals = new Map<number, Map<string, number>>();
  for (const s of sessions) {
    let from = (s.end - s.seconds) * 1000;
    const to = s.end * 1000;
    while (from < to) {
      const day = startOfDay(from);
      // Next midnight by the calendar: days are 23 or 25 hours long when the clocks change.
      const d = new Date(day);
      const until = Math.min(to, new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1).getTime());
      const games = totals.get(day) ?? new Map<string, number>();
      games.set(s.gameId, (games.get(s.gameId) ?? 0) + (until - from) / 1000);
      totals.set(day, games);
      from = until;
    }
  }
  return totals;
}

function useStats(sessions: Session[]) {
  return useMemo(() => {
    const totals = perDay(sessions);
    const today = startOfDay(Date.now());
    const days: Day[] = [];
    for (let i = DAYS - 1; i >= 0; i--) {
      const start = startOfDay(today - i * DAY_MS + DAY_MS / 2);
      const games = totals.get(start);
      let topGame: string | null = null;
      let secs = 0;
      let best = 0;
      for (const [id, v] of games ?? []) {
        secs += v;
        if (v > best) [best, topGame] = [v, id];
      }
      days.push({ start, secs, topGame });
    }

    // Weeks start on Monday, as in Italy.
    const weekday = (new Date(today).getDay() + 6) % 7;
    const weekStart = today - weekday * DAY_MS;
    const sumBetween = (from: number, to: number) =>
      [...totals].filter(([d]) => d >= from && d < to).reduce((acc, [, g]) => acc + [...g.values()].reduce((a, b) => a + b, 0), 0);
    const thisWeek = sumBetween(weekStart, today + DAY_MS);
    const lastWeek = sumBetween(weekStart - 7 * DAY_MS, weekStart);

    let streak = 0;
    let cursor = totals.has(today) ? today : today - DAY_MS;
    while (totals.has(startOfDay(cursor + DAY_MS / 2))) {
      streak++;
      cursor -= DAY_MS;
    }

    const byGame = new Map<string, number>();
    for (const s of sessions) byGame.set(s.gameId, (byGame.get(s.gameId) ?? 0) + s.seconds);
    const top = [...byGame].sort((a, b) => b[1] - a[1]).slice(0, 6);
    const total = [...byGame.values()].reduce((a, b) => a + b, 0);

    return { days, thisWeek, lastWeek, streak, top, total, played: byGame.size };
  }, [sessions]);
}

export function Stats() {
  const games = useApp((s) => s.games);
  const profile = useApp(activeProfileOf);
  const open = useApp((s) => s.open);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const [sessions, setSessions] = useState<Session[] | null>(null);

  useEffect(() => {
    setBackdrop(null);
    void api.getSessions().then(setSessions).catch(() => setSessions([]));
  }, [setBackdrop, profile?.id]);

  const gameById = useMemo(() => new Map(games.map((g) => [g.id, g])), [games]);
  // Time in apps (Netflix, Spotify…) isn't play time.
  const played = useMemo(() => (sessions ?? []).filter((s) => gameById.get(s.gameId)?.store !== "app"), [sessions, gameById]);
  const stats = useStats(played);
  useScreenFocus(undefined, sessions !== null);
  useHints([{ button: "A", label: "Apri gioco" }]);

  if (sessions === null) return <div className="screen empty"><div className="spinner" /></div>;
  if (played.length === 0) {
    return (
      <div className="screen empty">
        <p className="empty-title">Ancora nessuna partita</p>
        <p className="empty-text">Gioca qualcosa da GameMode: qui vedrai quanto e cosa giochi.</p>
      </div>
    );
  }

  const delta = stats.thisWeek - stats.lastWeek;
  return (
    <div className="screen stats-screen">
      <div className="stats-scroll">
        <h1 className="screen-title">
          Statistiche {profile && <span className="row-count">{profile.name}</span>}
        </h1>

        <div className="stat-tiles">
          <StatTile
            label="Questa settimana"
            value={formatPlaytime(Math.round(stats.thisWeek))}
            note={
              stats.lastWeek > 0 || stats.thisWeek > 0
                ? `${delta >= 0 ? "+" : "−"}${formatPlaytime(Math.abs(Math.round(delta)))} rispetto alla scorsa`
                : undefined
            }
          />
          <StatTile label="Tempo totale" value={formatPlaytime(stats.total)} />
          <StatTile label="Giochi giocati" value={String(stats.played)} />
          <StatTile label="Giorni di fila" value={String(stats.streak)} note={stats.streak > 0 ? "serie in corso" : "riparti oggi"} />
        </div>

        <section className="stats-section">
          <h2 className="row-title stats-title">Ultimi 14 giorni</h2>
          <DayChart days={stats.days} gameById={gameById} />
        </section>

        <section className="stats-section">
          <h2 className="row-title stats-title">I più giocati</h2>
          <div className="top-list">
            {stats.top.map(([id, secs]) => {
              const game = gameById.get(id);
              return (
                <button
                  key={id}
                  type="button"
                  className="top-row"
                  data-nav
                  disabled={!game}
                  onClick={() => game && open({ name: "detail", gameId: id })}
                >
                  <span className="top-name">{game?.name ?? "Gioco disinstallato"}</span>
                  <span className="top-track">
                    <span className="top-bar" style={{ width: `${(secs / stats.top[0][1]) * 100}%` }} />
                  </span>
                  <span className="top-value">{formatPlaytime(secs)}</span>
                </button>
              );
            })}
          </div>
        </section>
      </div>
    </div>
  );
}

function StatTile({ label, value, note }: { label: string; value: string; note?: string }) {
  return (
    <div className="stat-tile">
      <span className="stat-tile-label">{label}</span>
      <span className="stat-tile-value">{value}</span>
      {note && <span className="stat-tile-note">{note}</span>}
    </div>
  );
}

const weekdayFmt = new Intl.DateTimeFormat("it", { weekday: "short" });
const dateFmt = new Intl.DateTimeFormat("it", { weekday: "long", day: "numeric", month: "long" });

/** Clean tick step in hours for a maximum of `maxHours`. */
function tickStep(maxHours: number) {
  for (const step of [0.5, 1, 2, 3, 4, 6]) if (maxHours / step <= 4) return step;
  return 12;
}

/**
 * Daily playtime columns. The whole chart is one focusable control: left and
 * right move the highlighted day, like scrubbing with a mouse.
 */
function DayChart({ days, gameById }: { days: Day[]; gameById: Map<string, Game> }) {
  const [selected, setSelected] = useState(days.length - 1);
  const ref = useRef<HTMLButtonElement>(null);
  const maxHours = Math.max(0.5, ...days.map((d) => d.secs / 3600));
  const step = tickStep(maxHours);
  const top = Math.ceil(maxHours / step) * step;
  const ticks = Array.from({ length: Math.round(top / step) + 1 }, (_, i) => i * step);
  const day = days[selected];
  const today = startOfDay(Date.now());

  useEffect(() => {
    const el = ref.current;
    const listener = (e: Event) =>
      setSelected((s) => Math.min(days.length - 1, Math.max(0, s + (e as CustomEvent<number>).detail)));
    el?.addEventListener("navadjust", listener);
    return () => el?.removeEventListener("navadjust", listener);
  }, [days.length]);

  return (
    <button
      ref={ref}
      type="button"
      className="day-chart"
      data-nav
      data-nav-capture="x"
      data-nav-autofocus
      aria-label="Ore di gioco degli ultimi 14 giorni, usa sinistra e destra per scorrere">
      <span className="day-chart-plot">
        {ticks.map((t) => (
          <span key={t} className="day-grid" style={{ bottom: `${(t / top) * 100}%` }}>
            <span className="day-tick">{t === 0 ? "0" : `${t} h`}</span>
          </span>
        ))}
        <span className="day-columns">
          {days.map((d, i) => (
            <span
              key={d.start}
              className={`day-col ${i === selected ? "is-selected" : ""}`}
              onMouseEnter={() => setSelected(i)}
            >
              <span className="day-bar" style={{ height: `${(d.secs / 3600 / top) * 100}%` }} />
            </span>
          ))}
        </span>
        <span
          className="day-tooltip"
          style={{
            left: `${((selected + 0.5) / days.length) * 100}%`,
            // Keep the box inside the chart near its edges.
            transform: `translateX(${selected < 2 ? -15 : selected > days.length - 3 ? -85 : -50}%)`,
          }}
        >
          <span className="day-tooltip-date">{dayTitle(day.start, today)}</span>
          <span className="day-tooltip-value">{day.secs > 0 ? formatPlaytime(Math.round(day.secs)) : "nessuna partita"}</span>
          {day.topGame && <span className="day-tooltip-game">{gameById.get(day.topGame)?.name ?? "Gioco disinstallato"}</span>}
        </span>
      </span>
      <span className="day-labels">
        {days.map((d) => (
          <span key={d.start} className="day-label">
            {d.start === today ? "oggi" : weekdayFmt.format(d.start).slice(0, 3)}
          </span>
        ))}
      </span>
    </button>
  );
}

function dayTitle(start: number, today: number) {
  if (start === today) return "Oggi";
  if (start === startOfDay(today - DAY_MS / 2)) return "Ieri";
  const text = dateFmt.format(start);
  return text.charAt(0).toUpperCase() + text.slice(1);
}
