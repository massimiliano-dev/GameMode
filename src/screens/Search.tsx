import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { CatalogCard } from "../components/CatalogCard";
import { GameCard } from "../components/GameCard";
import { useHints } from "../components/common";
import { api } from "../lib/api";
import type { CatalogItem, Game } from "../lib/types";
import { useApp, visibleGames } from "../store/app";

const RESULT_LIMIT = 30;

/** Lowercase, accents stripped, so "pokemon" finds "Pokémon". */
function normalize(s: string) {
  return s.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

function matches(name: string, words: string[]) {
  const n = normalize(name);
  return words.every((w) => n.includes(w));
}

export function Search() {
  const games = useApp((s) => s.games);
  const open = useApp((s) => s.open);
  const installFromCatalog = useApp((s) => s.installFromCatalog);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const setBackdropArt = useApp((s) => s.setBackdropArt);
  const setKeyboardField = useApp((s) => s.setKeyboardField);
  const [query, setQuery] = useState("");
  const [catalog, setCatalog] = useState<CatalogItem[]>([]);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    void Promise.all([api.getCatalog(false), api.getCloudCatalog(false).catch(() => [])])
      .then(([c, cloud]) => setCatalog([...c.items, ...cloud]))
      .catch(() => {});
    const field = input.current;
    if (!field) return;
    field.focus();
    // Coming from a controller: go straight to the on-screen keyboard.
    if (document.body.classList.contains("no-cursor")) setKeyboardField(field);
  }, [setKeyboardField]);

  useHints([
    { button: "A", label: "Seleziona" },
    { button: "B", label: "Indietro" },
  ]);

  const words = useMemo(() => normalize(query).split(/\s+/).filter(Boolean), [query]);
  const installed = useMemo(
    () => (words.length ? visibleGames(games).filter((g) => matches(g.name, words)).slice(0, RESULT_LIMIT) : []),
    [games, words],
  );
  const available = useMemo(
    () => (words.length ? catalog.filter((i) => matches(i.name, words)).slice(0, RESULT_LIMIT) : []),
    [catalog, words],
  );

  const onGameFocus = useCallback((g: Game) => setBackdrop(g.id), [setBackdrop]);
  const onGameSelect = useCallback((g: Game) => open({ name: "detail", gameId: g.id }), [open]);
  const onItemFocus = useCallback(
    (i: CatalogItem) => setBackdropArt({ key: i.id, hero: i.hero, cover: i.cover, store: i.store }),
    [setBackdropArt],
  );
  const onItemSelect = useCallback((i: CatalogItem) => void installFromCatalog(i.id, i.name), [installFromCatalog]);

  return (
    <div className="screen search">
      <header className="search-head">
        <input
          ref={input}
          className="text-input search-input"
          data-nav
          data-nav-id="search-input"
          aria-label="Cerca un gioco"
          placeholder="Cerca un gioco…"
          value={query}
          spellCheck={false}
          onChange={(e) => setQuery(e.target.value)}
        />
      </header>
      <div className="search-results">
        {words.length > 0 && installed.length === 0 && available.length === 0 && (
          <p className="empty-text">Nessun gioco trovato per “{query}”.</p>
        )}
        {installed.length > 0 && (
          <section className="search-section">
            <h2 className="row-title">Nella tua libreria</h2>
            <div className="row-scroll" data-nav-group="search-installed">
              {installed.map((g) => (
                <GameCard key={g.id} game={g} navId={`search:${g.id}`} onFocus={onGameFocus} onSelect={onGameSelect} />
              ))}
            </div>
          </section>
        )}
        {available.length > 0 && (
          <section className="search-section">
            <h2 className="row-title">Da installare</h2>
            <div className="row-scroll" data-nav-group="search-catalog">
              {available.map((i) => (
                <CatalogCard key={i.id} item={i} autofocus={false} onFocus={onItemFocus} onSelect={onItemSelect} />
              ))}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
