import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";

import { Glyph, StoreBadge, useHints, useScreenFocus } from "../components/common";
import { useAction } from "../input/dispatcher";
import { forgetGroup } from "../input/focus";
import { api } from "../lib/api";
import { formatPlaytime } from "../lib/format";
import type { Catalog, CatalogItem } from "../lib/types";
import { useApp } from "../store/app";

type Filter = "all" | "steam" | "gamepass";
const FILTERS: { key: Filter; label: string }[] = [
  { key: "all", label: "Tutti" },
  { key: "steam", label: "La tua libreria Steam" },
  { key: "gamepass", label: "PC Game Pass" },
];

const STEAM_NOTICE: Record<Exclude<Catalog["steam"], "ok">, string> = {
  notInstalled: "Steam non è installato su questo PC.",
  needsKey:
    "Per vedere i giochi Steam che possiedi ma non hai installato, inserisci la tua chiave Web API di Steam nelle Impostazioni.",
  noAccount: "Accedi a Steam almeno una volta su questo PC per vedere la tua libreria.",
  failed:
    "Non riesco a leggere la libreria Steam: controlla la chiave Web API e che nel profilo Steam i dettagli dei giochi siano visibili.",
};

export function Downloads() {
  const installFromCatalog = useApp((s) => s.installFromCatalog);
  const setBackdropArt = useApp((s) => s.setBackdropArt);
  const setTab = useApp((s) => s.setTab);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<Filter>("all");
  const [focused, setFocused] = useState<CatalogItem | null>(null);
  const focusedRef = useRef<CatalogItem | null>(null);

  const load = useCallback(async (force: boolean) => {
    setLoading(true);
    try {
      setCatalog(await api.getCatalog(force));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    setBackdropArt(null);
    void load(false);
  }, [load, setBackdropArt]);

  const shown = useMemo(
    () => (catalog?.items ?? []).filter((i) => filter === "all" || i.source === filter),
    [catalog, filter],
  );

  useScreenFocus(undefined, !loading);
  useHints([
    { button: "A", label: "Installa" },
    { button: "LT", alt: "RT", label: "Filtro" },
    { button: "X", label: "Aggiorna" },
  ]);

  useAction((action, repeat) => {
    if (repeat) return;
    if (action === "lt" || action === "rt") {
      const i = FILTERS.findIndex((f) => f.key === filter);
      const next = FILTERS[(i + (action === "rt" ? 1 : FILTERS.length - 1)) % FILTERS.length].key;
      setFilter(next);
      forgetGroup("catalog");
      requestAnimationFrame(() => document.querySelector<HTMLElement>(".catalog-grid [data-nav]")?.focus());
      return true;
    }
    if (action === "x") {
      void load(true);
      return true;
    }
  });

  const onFocus = useCallback(
    (item: CatalogItem) => {
      focusedRef.current = item;
      setFocused(item);
      setBackdropArt({ key: item.id, hero: item.hero, cover: item.cover, store: item.store });
    },
    [setBackdropArt],
  );
  const onSelect = useCallback((item: CatalogItem) => void installFromCatalog(item.id, item.name), [installFromCatalog]);

  const steamNotice = catalog && catalog.steam !== "ok" && filter !== "gamepass" ? STEAM_NOTICE[catalog.steam] : null;

  return (
    <div className="screen downloads">
      <header className="library-head">
        <h1 className="screen-title">
          Scarica <span className="row-count">{shown.length}</span>
        </h1>
        <div className="chips">
          <Glyph button="LT" />
          {FILTERS.map((f) => (
            <span key={f.key} className={`chip ${f.key === filter ? "is-active" : ""}`}>
              {f.label}
            </span>
          ))}
          <Glyph button="RT" />
        </div>
        {focused && (
          <span className="sort-label catalog-focus">
            <StoreBadge store={focused.store} />
            {focused.source === "gamepass"
              ? "Incluso in PC Game Pass"
              : focused.playtimeSecs > 0
                ? `Giocato ${formatPlaytime(focused.playtimeSecs)}`
                : "Mai giocato"}
          </span>
        )}
      </header>

      {steamNotice && (
        <div className="notice">
          <span>{steamNotice}</span>
          {catalog?.steam === "needsKey" && (
            <button type="button" className="btn" data-nav onClick={() => setTab("settings")}>
              Apri Impostazioni
            </button>
          )}
        </div>
      )}
      {catalog && !catalog.gamepassOk && filter !== "steam" && (
        <div className="notice">Catalogo PC Game Pass non raggiungibile: controlla la connessione e premi X per riprovare.</div>
      )}

      {loading && !catalog ? (
        <div className="screen empty">
          <div className="spinner" />
          <p className="empty-text">Carico i cataloghi…</p>
        </div>
      ) : shown.length === 0 ? (
        <p className="empty-text library-empty">Niente da installare qui.</p>
      ) : (
        <div className="library-grid catalog-grid" data-nav-group="catalog">
          {shown.map((item, i) => (
            <CatalogCard key={item.id} item={item} autofocus={i === 0} onFocus={onFocus} onSelect={onSelect} />
          ))}
        </div>
      )}
    </div>
  );
}

const CatalogCard = memo(function CatalogCard(props: {
  item: CatalogItem;
  autofocus: boolean;
  onFocus: (item: CatalogItem) => void;
  onSelect: (item: CatalogItem) => void;
}) {
  const { item } = props;
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
      </span>
      <span className="install-pill" aria-hidden>
        ↓ Installa
      </span>
    </button>
  );
});
