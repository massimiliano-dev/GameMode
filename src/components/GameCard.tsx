import { memo, useState } from "react";

import { artUrl } from "../lib/api";
import type { Game } from "../lib/types";
import { useApp } from "../store/app";
import { StoreBadge } from "./common";

interface Props {
  game: Game;
  navId: string;
  onFocus?: (game: Game) => void;
  onSelect: (game: Game) => void;
  playing?: boolean;
  /** Frozen by Quick Resume. */
  paused?: boolean;
  autofocus?: boolean;
}

export const GameCard = memo(function GameCard({ game, navId, onFocus, onSelect, playing, paused, autofocus }: Props) {
  const update = useApp((s) => s.gameUpdates[game.id]);
  const removing = useApp((s) => (game.id in s.uninstalling ? (s.uninstalling[game.id] ?? -1) : null));
  return (
    <button
      type="button"
      className="game-card"
      data-nav
      data-nav-id={navId}
      data-nav-autofocus={autofocus || undefined}
      onFocus={() => onFocus?.(game)}
      onClick={() => onSelect(game)}
      aria-label={game.name}
    >
      <Cover game={game} />
      <span className="game-card-meta">
        <StoreBadge store={game.store} compact />
        {game.favorite && <span className="fav-star" aria-label="Preferito">★</span>}
        {playing && <span className="playing-pill">In gioco</span>}
        {!playing && paused && <span className="playing-pill is-paused">In pausa</span>}
        {removing !== null && (
          <span className="update-pill is-removing">{removing >= 0 ? `Rimozione ${removing}%` : "Rimozione…"}</span>
        )}
        {!playing && !paused && removing === null && update && (
          <span className={`update-pill ${update.state === "available" ? "" : "is-active"}`}>
            {update.state === "available"
              ? "Aggiornamento"
              : update.progress != null
                ? `↓ ${Math.round(update.progress * 100)}%`
                : "In aggiornamento"}
          </span>
        )}
      </span>
    </button>
  );
});

/** Portrait cover, or a typographic placeholder when no art is available. */
export function Cover({ game }: { game: Game }) {
  const [failed, setFailed] = useState(false);
  const src = artUrl(game.artwork.cover, game.artworkCheckedAt);
  if (!src || failed) {
    return (
      <span className={`cover cover-placeholder store-tint-${game.store}`}>
        <span className="cover-placeholder-name">{game.name}</span>
      </span>
    );
  }
  // App icons are square: shown whole, never cropped to the poster shape.
  const icon = game.store === "app" ? " cover-icon" : "";
  return (
    <img className={`cover${icon}`} src={src} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setFailed(true)} />
  );
}
