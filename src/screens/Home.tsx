import { useCallback, useMemo } from "react";

import { Cover, GameCard } from "../components/GameCard";
import { StoreBadge, useHints, useScreenFocus } from "../components/common";
import { useAction } from "../input/dispatcher";
import { artUrl } from "../lib/api";
import { formatLastPlayed, formatPlaytime, STORE_LABEL, STORES } from "../lib/format";
import type { Collection, Game } from "../lib/types";
import { useApp, visibleGames } from "../store/app";

interface Row {
  key: string;
  title: string;
  games: Game[];
}

const ROW_LIMIT = 14;

function buildRows(all: Game[], collections: Collection[]): Row[] {
  const games = visibleGames(all);
  const rows: Row[] = [];
  const recent = games
    .filter((g) => g.lastPlayed)
    .sort((a, b) => (b.lastPlayed ?? 0) - (a.lastPlayed ?? 0))
    .slice(0, ROW_LIMIT);
  if (recent.length) rows.push({ key: "recent", title: "Continua a giocare", games: recent });

  const favorites = games.filter((g) => g.favorite);
  if (favorites.length) rows.push({ key: "favorites", title: "Preferiti", games: favorites });

  for (const c of collections) {
    const list = c.gameIds.map((id) => games.find((g) => g.id === id)).filter((g): g is Game => !!g);
    if (list.length) rows.push({ key: `c-${c.id}`, title: c.name, games: list });
  }

  // On first launch every game shares the same timestamp: nothing is "new".
  const oldest = Math.min(...games.map((g) => g.addedAt));
  const added = games
    .filter((g) => g.addedAt > oldest + 3600)
    .sort((a, b) => b.addedAt - a.addedAt)
    .slice(0, ROW_LIMIT);
  if (added.length) rows.push({ key: "added", title: "Aggiunti di recente", games: added });

  for (const store of STORES) {
    const list = games.filter((g) => g.store === store);
    if (list.length) rows.push({ key: store, title: STORE_LABEL[store], games: list });
  }
  if (!rows.length && games.length) rows.push({ key: "all", title: "Tutti i giochi", games });
  return rows;
}

export function Home({ focusId }: { focusId?: string }) {
  const games = useApp((s) => s.games);
  const collections = useApp((s) => s.collections);
  const loaded = useApp((s) => s.loaded);
  const open = useApp((s) => s.open);
  const launch = useApp((s) => s.launch);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const featured = useApp((s) => s.games.find((g) => g.id === s.backdropId));
  const sessionGame = useApp((s) => s.session?.gameId);
  const rows = useMemo(() => buildRows(games, collections), [games, collections]);

  useScreenFocus(focusId, rows.length > 0);
  useHints([
    { button: "A", label: "Dettagli" },
    { button: "X", label: "Gioca" },
    { button: "Y", label: "Preferito" },
  ]);

  const onFocus = useCallback((g: Game) => setBackdrop(g.id), [setBackdrop]);
  const onSelect = useCallback((g: Game) => open({ name: "detail", gameId: g.id }), [open]);

  useAction((action, repeat) => {
    if (repeat || !featured) return;
    if (action === "x") {
      void launch(featured.id);
      return true;
    }
    if (action === "y") {
      void toggleFavorite(featured);
      return true;
    }
  });

  if (!rows.length) return <EmptyLibrary loaded={loaded} />;

  return (
    <div className="screen home">
      <section className="home-hero">{featured && <FeaturedInfo game={featured} />}</section>
      <div className="rows">
        {rows.map((row, rowIndex) => (
          <section key={row.key} className="row" data-nav-group={`row:${row.key}`}>
            <h2 className="row-title">
              {row.title}
              <span className="row-count">{row.games.length}</span>
            </h2>
            <div className="row-scroll">
              {row.games.map((g, i) => (
                <GameCard
                  key={g.id}
                  game={g}
                  navId={`${row.key}:${g.id}`}
                  onFocus={onFocus}
                  onSelect={onSelect}
                  playing={sessionGame === g.id}
                  autofocus={rowIndex === 0 && i === 0}
                />
              ))}
            </div>
          </section>
        ))}
      </div>
    </div>
  );
}

function FeaturedInfo({ game }: { game: Game }) {
  const logo = artUrl(game.artwork.logo, game.artworkCheckedAt);
  return (
    <div className="featured" key={game.id}>
      {logo ? (
        <img className="featured-logo" src={logo} alt={game.name} />
      ) : (
        <h1 className="featured-title">{game.name}</h1>
      )}
      <div className="featured-meta">
        <StoreBadge store={game.store} />
        <span>{formatPlaytime(game.playtimeSecs)}</span>
        <span className="sep" />
        <span>Ultima partita: {formatLastPlayed(game.lastPlayed)}</span>
      </div>
    </div>
  );
}

function EmptyLibrary({ loaded }: { loaded: boolean }) {
  const rescan = useApp((s) => s.rescan);
  const scanning = useApp((s) => s.scanning);
  const setTab = useApp((s) => s.setTab);
  useScreenFocus(undefined, loaded);
  useHints([{ button: "A", label: "Seleziona" }]);

  if (!loaded || scanning) {
    return (
      <div className="screen empty">
        <div className="spinner" />
        <p className="empty-title">Cerco i tuoi giochi…</p>
        <p className="empty-text">Steam, Xbox, Epic, GOG, EA e Ubisoft</p>
      </div>
    );
  }
  return (
    <div className="screen empty">
      <div className="empty-art">
        <Cover game={{ id: "x", name: "?", store: "manual", artwork: {} } as Game} />
      </div>
      <p className="empty-title">Nessun gioco trovato</p>
      <p className="empty-text">
        Installa un gioco da uno store, oppure aggiungi un eseguibile a mano dalle impostazioni.
      </p>
      <div className="button-row">
        <button type="button" className="btn btn-primary" data-nav data-nav-autofocus onClick={() => void rescan()}>
          Cerca di nuovo
        </button>
        <button type="button" className="btn" data-nav onClick={() => setTab("settings")}>
          Impostazioni
        </button>
      </div>
    </div>
  );
}
