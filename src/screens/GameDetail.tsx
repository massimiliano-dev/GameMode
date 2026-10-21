import { useEffect } from "react";

import { Cover } from "../components/GameCard";
import { StoreBadge, useHints, useScreenFocus } from "../components/common";
import { api, artUrl } from "../lib/api";
import { formatLastPlayed, formatPlaytime, SHOP_LABEL } from "../lib/format";
import { useApp } from "../store/app";

export function GameDetail({ gameId }: { gameId: string }) {
  const game = useApp((s) => s.games.find((g) => g.id === gameId));
  const session = useApp((s) => s.session);
  const launch = useApp((s) => s.launch);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const toggleHidden = useApp((s) => s.toggleHidden);
  const openStore = useApp((s) => s.openStore);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const back = useApp((s) => s.back);

  useEffect(() => setBackdrop(gameId), [gameId, setBackdrop]);
  useScreenFocus(undefined, !!game);
  useHints([
    { button: "A", label: "Seleziona" },
    { button: "B", label: "Indietro" },
  ]);

  if (!game) {
    return (
      <div className="screen empty">
        <p className="empty-title">Gioco non più disponibile</p>
        <button type="button" className="btn" data-nav data-nav-autofocus onClick={back}>
          Indietro
        </button>
      </div>
    );
  }

  const logo = artUrl(game.artwork.logo, game.artworkCheckedAt);
  const mine = session?.gameId === game.id ? session : null;

  return (
    <div className="screen detail">
      <div className="detail-main">
        {logo ? <img className="detail-logo" src={logo} alt={game.name} /> : <h1 className="detail-title">{game.name}</h1>}
        <div className="featured-meta">
          <StoreBadge store={game.store} />
          {game.favorite && <span className="fav-star">★ Preferito</span>}
        </div>

        <div className="button-row detail-actions" data-nav-group="actions">
          {mine ? (
            <>
              <button type="button" className="btn btn-primary btn-play" data-nav data-nav-autofocus onClick={() => void api.resumeGame()}>
                {mine.phase === "launching" ? "Avvio in corso…" : "Torna al gioco"}
              </button>
              <button type="button" className="btn" data-nav onClick={() => void api.closeGame()}>
                Chiudi gioco
              </button>
            </>
          ) : (
            <button type="button" className="btn btn-primary btn-play" data-nav data-nav-autofocus onClick={() => void launch(game.id)}>
              <span className="play-icon" aria-hidden>▶</span> Gioca
            </button>
          )}
          <button type="button" className="btn" data-nav onClick={() => void toggleFavorite(game)}>
            {game.favorite ? "★ Rimuovi dai preferiti" : "☆ Aggiungi ai preferiti"}
          </button>
          {SHOP_LABEL[game.store] && (
            <button type="button" className="btn" data-nav onClick={() => void openStore(game.store, game.id)}>
              Pagina nello store
            </button>
          )}
          <button
            type="button"
            className="btn"
            data-nav
            onClick={() => {
              void toggleHidden(game);
              if (!game.hidden) back();
            }}
          >
            {game.hidden ? "Mostra nella libreria" : "Nascondi"}
          </button>
        </div>

        <dl className="stats">
          <div className="stat">
            <dt>Tempo di gioco</dt>
            <dd>{formatPlaytime(game.playtimeSecs)}</dd>
          </div>
          <div className="stat">
            <dt>Ultima partita</dt>
            <dd>{formatLastPlayed(game.lastPlayed)}</dd>
          </div>
          {game.installDir && (
            <div className="stat stat-wide">
              <dt>Installato in</dt>
              <dd className="mono">{game.installDir}</dd>
            </div>
          )}
        </dl>
      </div>
      <div className="detail-cover">
        <Cover game={game} />
      </div>
    </div>
  );
}
