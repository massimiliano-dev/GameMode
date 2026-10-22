import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { CatalogCard } from "../components/CatalogCard";

import { Glyph, StoreBadge, useHints, useScreenFocus } from "../components/common";
import { useAction } from "../input/dispatcher";
import { forgetGroup } from "../input/focus";
import { api } from "../lib/api";
import { formatPlaytime } from "../lib/format";
import type { Catalog, CatalogItem, Deals } from "../lib/types";
import { useApp } from "../store/app";

type Filter = "all" | CatalogItem["source"];
const ALL_FILTERS: { key: Filter; label: string }[] = [
  { key: "all", label: "Tutti" },
  { key: "steam", label: "Steam" },
  { key: "epic", label: "Epic" },
  { key: "gog", label: "GOG" },
  { key: "gamepass", label: "PC Game Pass" },
  { key: "cloud", label: "Cloud" },
  { key: "deal", label: "Offerte" },
];
/** "Tutti" lists what can be installed; cloud games and offers have their own tabs. */
const IN_ALL = new Set<CatalogItem["source"]>(["steam", "epic", "gog", "gamepass"]);

function dealItems(deals: Deals | null): CatalogItem[] {
  return (deals?.items ?? []).map((d) => ({
    id: `deal:${d.appid}`,
    source: "deal",
    store: "steam",
    name: d.name,
    cover: d.cover,
    hero: d.hero,
    playtimeSecs: 0,
    discount: d.discount,
    price: d.price,
    initial: d.initial,
  }));
}

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
  const [deals, setDeals] = useState<Deals | null>(null);
  const [cloud, setCloud] = useState<CatalogItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [filter, setFilter] = useState<Filter>("all");
  const [focused, setFocused] = useState<CatalogItem | null>(null);
  const focusedRef = useRef<CatalogItem | null>(null);

  const load = useCallback(async (force: boolean) => {
    setLoading(true);
    // Thousands of titles: arrives on its own, after the rest.
    void api.getCloudCatalog(force).then(setCloud).catch(() => {});
    try {
      const [c, d] = await Promise.all([api.getCatalog(force), api.wishlistDeals(force).catch(() => null)]);
      setCatalog(c);
      setDeals(d);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    setBackdropArt(null);
    void load(false);
  }, [load, setBackdropArt]);

  const items = useMemo(() => [...(catalog?.items ?? []), ...cloud, ...dealItems(deals)], [catalog, cloud, deals]);
  const filters = useMemo(
    () => ALL_FILTERS.filter((f) => f.key === "all" || items.some((i) => i.source === f.key)),
    [items],
  );
  const shown = useMemo(
    () => items.filter((i) => (filter === "all" ? IN_ALL.has(i.source) : i.source === filter)),
    [items, filter],
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
      const i = filters.findIndex((f) => f.key === filter);
      const next = filters[(i + (action === "rt" ? 1 : filters.length - 1)) % filters.length].key;
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
  const toast = useApp((s) => s.toast);
  const onSelect = useCallback(
    (item: CatalogItem) => {
      if (item.source === "deal") {
        void api.openLink(`steam://store/${item.id.slice("deal:".length)}`);
        toast(`${item.name} a ${item.price} nel negozio Steam. Premi Guide per tornare qui`);
      } else {
        void installFromCatalog(item.id, item.name);
      }
    },
    [installFromCatalog, toast],
  );

  const steamNotice = catalog && catalog.steam !== "ok" && (filter === "all" || filter === "steam") ? STEAM_NOTICE[catalog.steam] : null;

  return (
    <div className="screen downloads">
      <header className="library-head">
        <h1 className="screen-title">
          Scarica <span className="row-count">{shown.length}</span>
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
        {focused && (
          <span className="sort-label catalog-focus">
            <StoreBadge store={focused.store} />
            {describe(focused)}
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
      {catalog && !catalog.gamepassOk && (filter === "all" || filter === "gamepass") && (
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

function describe(item: CatalogItem) {
  switch (item.source) {
    case "gamepass":
      return "Incluso in PC Game Pass";
    case "cloud":
      return "Gioca in streaming con Xbox Cloud Gaming";
    case "deal":
      return `In offerta: ${item.initial} → ${item.price}`;
    default:
      return item.playtimeSecs > 0 ? `Giocato ${formatPlaytime(item.playtimeSecs)}` : "Mai giocato";
  }
}
