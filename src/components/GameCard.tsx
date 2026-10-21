import { memo, useState } from "react";

import { artUrl } from "../lib/api";
import type { Game } from "../lib/types";
import { StoreBadge } from "./common";

interface Props {
  game: Game;
  navId: string;
  onFocus?: (game: Game) => void;
  onSelect: (game: Game) => void;
  playing?: boolean;
  autofocus?: boolean;
}

export const GameCard = memo(function GameCard({ game, navId, onFocus, onSelect, playing, autofocus }: Props) {
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
  return (
    <img className="cover" src={src} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setFailed(true)} />
  );
}
