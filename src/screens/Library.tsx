import { useCallback, useMemo, useRef, useState } from "react";

import { GameCard } from "../components/GameCard";
import { Glyph, useHints, useScreenFocus } from "../components/common";
import { useAction } from "../input/dispatcher";
import { ensureFocus, forgetGroup } from "../input/focus";
import { STORE_LABEL, STORES } from "../lib/format";
import type { Game, Store } from "../lib/types";
import { useApp, visibleGames } from "../store/app";

type Sort = "name" | "recent" | "playtime";
const SORTS: { key: Sort; label: string }[] = [
  { key: "name", label: "A–Z" },
  { key: "recent", label: "Giocati di recente" },
  { key: "playtime", label: "Più giocati" },
];

const SORTERS: Record<Sort, (a: Game, b: Game) => number> = {
  name: (a, b) => a.name.localeCompare(b.name, "it"),
  recent: (a, b) => (b.lastPlayed ?? 0) - (a.lastPlayed ?? 0) || a.name.localeCompare(b.name, "it"),
  playtime: (a, b) => b.playtimeSecs - a.playtimeSecs || a.name.localeCompare(b.name, "it"),
};

const MAX_GENRE_FILTERS = 6;

/** Genres shared by at least two games, most common first. */
function topGenres(games: Game[]): string[] {
  const counts = new Map<string, number>();
  for (const g of games) for (const genre of g.meta?.genres ?? []) counts.set(genre, (counts.get(genre) ?? 0) + 1);
  return [...counts]
    .filter(([, n]) => n >= 2)
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0], "it"))
    .slice(0, MAX_GENRE_FILTERS)
    .map(([genre]) => genre);
}

/** The top genres, plus the selected one if new cards pushed it out of the top. */
function genreFilters(games: Game[], filter: string): string[] {
  const genres = topGenres(games);
  const selected = filter.startsWith("g:") ? filter.slice(2) : null;
  return selected && !genres.includes(selected) ? [...genres, selected] : genres;
}

export function Library({ focusId }: { focusId?: string }) {
  const all = useApp((s) => s.games);
  const open = useApp((s) => s.open);
  const launch = useApp((s) => s.launch);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const sessionGame = useApp((s) => s.session?.gameId);
  const suspended = useApp((s) => s.suspended);
  const collections = useApp((s) => s.collections);
  // "all", a store, `c:<collection id>` or `g:<genre>`.
  const [filter, setFilter] = useState<string>("all");
  const [sort, setSort] = useState<Sort>("name");
  const focused = useRef<Game | null>(null);

  const games = useMemo(() => visibleGames(all), [all]);
  const filters = useMemo(
    () => [
      { key: "all", label: "Tutti" },
      ...STORES.filter((s) => games.some((g) => g.store === s)).map((s) => ({ key: s as string, label: STORE_LABEL[s] })),
      ...collections.filter((c) => c.gameIds.length).map((c) => ({ key: `c:${c.id}`, label: c.name })),
      ...genreFilters(games, filter).map((genre) => ({ key: `g:${genre}`, label: genre })),
    ],
    [games, collections, filter],
  );
  const shown = useMemo(() => {
    const collection = filter.startsWith("c:") ? collections.find((c) => `c:${c.id}` === filter) : null;
    const genre = filter.startsWith("g:") ? filter.slice(2) : null;
    const matches = (g: Game) => {
      if (filter === "all") return true;
      if (collection) return collection.gameIds.includes(g.id);
      if (genre) return (g.meta?.genres ?? []).includes(genre);
      return g.store === (filter as Store);
    };
    return games.filter(matches).sort(SORTERS[sort]);
  }, [games, collections, filter, sort]);

  useScreenFocus(focusId, shown.length > 0);
  useHints([
    { button: "A", label: "Dettagli" },
    { button: "X", label: "Gioca" },
    { button: "Y", label: "Ordina" },
    { button: "LT", alt: "RT", label: "Filtro" },
  ]);

  const regroup = () => {
    // The grid content changes: start again from its first card.
    forgetGroup("grid");
    requestAnimationFrame(() => {
      const first = document.querySelector<HTMLElement>(".library-grid [data-nav]");
      first?.focus({ preventScroll: true });
      first?.scrollIntoView({ block: "nearest" });
      ensureFocus();
    });
  };

  useAction((action, repeat) => {
    if (repeat) return;
    if (action === "lt" || action === "rt") {
      const i = filters.findIndex((f) => f.key === filter);
      const next = filters[(i + (action === "rt" ? 1 : filters.length - 1)) % filters.length].key;
      setFilter(next);
      regroup();
      return true;
    }
    if (action === "y") {
      setSort(SORTS[(SORTS.findIndex((s) => s.key === sort) + 1) % SORTS.length].key);
      regroup();
      return true;
    }
    if (action === "x" && focused.current) {
      void launch(focused.current.id);
      return true;
    }
  });

  const onFocus = useCallback(
    (g: Game) => {
      focused.current = g;
      setBackdrop(g.id);
    },
    [setBackdrop],
  );
  const onSelect = useCallback((g: Game) => open({ name: "detail", gameId: g.id }), [open]);

  return (
    <div className="screen library">
      <header className="library-head">
        <h1 className="screen-title">
          Libreria <span className="row-count">{shown.length}</span>
        </h1>
        <div className="chips">
          <Glyph button="LT" />
          {filters.map((f) => (
            <span key={f.key} className={`chip ${f.key === filter ? "is-active" : ""}`}>
              {f.label}
            </span>
          ))}
          <Glyph button="RT" />
        </div>
        <span className="sort-label">
          <Glyph button="Y" /> {SORTS.find((s) => s.key === sort)!.label}
        </span>
      </header>
      {shown.length === 0 ? (
        <p className="empty-text library-empty">Nessun gioco in questa libreria.</p>
      ) : (
        <div className="library-grid" data-nav-group="grid">
          {shown.map((g, i) => (
            <GameCard
              key={g.id}
              game={g}
              navId={`grid:${g.id}`}
              onFocus={onFocus}
              onSelect={onSelect}
              playing={sessionGame === g.id}
              paused={suspended.some((p) => p.gameId === g.id)}
                  autofocus={i === 0}
            />
          ))}
        </div>
      )}
    </div>
  );
}
