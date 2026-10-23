import { memo, useState } from "react";

import { artUrl } from "../lib/api";
import type { Game } from "../lib/types";

interface Props {
  app: Game;
  navId: string;
  onFocus?: (app: Game) => void;
  onSelect: (app: Game) => void;
  /** Open right now (its session is in the foreground). */
  running?: boolean;
  autofocus?: boolean;
}

/** A square tile: the app's icon with its name underneath. */
export const AppCard = memo(function AppCard({ app, navId, onFocus, onSelect, running, autofocus }: Props) {
  return (
    <button
      type="button"
      className="app-card"
      data-nav
      data-nav-id={navId}
      data-nav-autofocus={autofocus || undefined}
      onFocus={() => onFocus?.(app)}
      onClick={() => onSelect(app)}
      aria-label={app.name}
    >
      <span className="app-tile">
        <AppIcon app={app} />
        {app.favorite && <span className="fav-star app-star" aria-label="Preferito">★</span>}
        {running && <span className="playing-pill app-pill">Aperta</span>}
      </span>
      <span className="app-name">{app.name}</span>
    </button>
  );
});

/** The cached icon, or the app's initial while there is none. */
export function AppIcon({ app }: { app: Game }) {
  const [failed, setFailed] = useState(false);
  const src = artUrl(app.artwork.cover, app.artworkCheckedAt);
  if (!src || failed) {
    return (
      <span className="app-icon app-icon-placeholder" aria-hidden>
        {initial(app.name)}
      </span>
    );
  }
  return <img className="app-icon" src={src} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setFailed(true)} />;
}

function initial(name: string) {
  const first = name.trim().match(/\p{L}|\p{N}/u);
  return first ? first[0].toUpperCase() : "·";
}
