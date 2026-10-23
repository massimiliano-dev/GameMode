import { memo, useState } from "react";

import { formatBytes } from "../lib/format";
import type { CatalogItem, GameUpdate } from "../lib/types";
import { StoreBadge } from "./common";

const ACTION_LABEL: Record<CatalogItem["source"], string> = {
  steam: "↓ Installa",
  epic: "↓ Installa",
  gog: "↓ Installa",
  gamepass: "↓ Installa",
  cloud: "▶ Gioca in cloud",
  deal: "Vedi offerta",
};

/** A game that isn't installed yet; pressing it starts the install (or opens its offer). */
export const CatalogCard = memo(function CatalogCard(props: {
  item: CatalogItem;
  autofocus: boolean;
  onFocus: (item: CatalogItem) => void;
  onSelect: (item: CatalogItem) => void;
  /** GameMode is downloading/installing it right now. */
  download?: GameUpdate;
}) {
  const { item, download } = props;
  const [failed, setFailed] = useState(false);
  return (
    <button
      type="button"
      className="game-card"
      data-nav
      data-nav-id={`catalog:${item.id}`}
      data-nav-autofocus={props.autofocus || undefined}
      onFocus={() => props.onFocus(item)}
      onClick={() => props.onSelect(item)}
      aria-label={`Installa ${item.name}`}
    >
      {item.cover && !failed ? (
        <img className="cover" src={item.cover} alt="" loading="lazy" decoding="async" draggable={false} onError={() => setFailed(true)} />
      ) : (
        <span className={`cover cover-placeholder store-tint-${item.store}`}>
          <span className="cover-placeholder-name">{item.name}</span>
        </span>
      )}
      <span className="game-card-meta">
        <StoreBadge store={item.store} compact />
        {item.source === "gamepass" && <span className="pass-pill">Game Pass</span>}
        {item.source === "cloud" && <span className="pass-pill">Cloud</span>}
        {item.discount ? <span className="discount-pill">−{item.discount}%</span> : null}
        {item.sizeBytes ? <span className="size-pill">{formatBytes(item.sizeBytes)}</span> : null}
      </span>
      <span className="install-pill" aria-hidden>
        {download ? downloadLabel(download) : ACTION_LABEL[item.source]}
      </span>
    </button>
  );
});

function downloadLabel(u: GameUpdate) {
  const pct = u.progress != null ? ` ${Math.round(u.progress * 100)}%` : "…";
  if (u.state === "installing") return `Installo${pct}`;
  if (u.state === "paused") return `In pausa${pct}`;
  return `↓${pct}`;
}
