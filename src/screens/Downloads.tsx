import { AnimatePresence } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { CatalogCard } from "../components/CatalogCard";
import { ConfirmDialog } from "../components/ConfirmDialog";

import { Glyph, StoreBadge, useHints, useScreenFocus } from "../components/common";
import { useAction } from "../input/dispatcher";
import { forgetGroup } from "../input/focus";
import { api, on } from "../lib/api";
import { formatBytes, formatPlaytime, updateProgress } from "../lib/format";
import type { Catalog, CatalogItem, Deals, DirectDone, Drive, GameUpdate } from "../lib/types";
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
  /** An install that doesn't fit on the drive it would go to, waiting for a yes. */
  const [tight, setTight] = useState<{ item: CatalogItem; drive: Drive } | null>(null);
  /** GameMode's own downloads (GOG, Microsoft Store), by catalog id. */
  const gameUpdates = useApp((s) => s.gameUpdates);
  const refreshGameUpdates = useApp((s) => s.refreshGameUpdates);
  const [stopping, setStopping] = useState<CatalogItem | null>(null);
  const downloading = useMemo(() => (catalog?.items ?? []).some((i) => gameUpdates[i.id]), [catalog, gameUpdates]);
  // Live progress while something downloads here.
  useEffect(() => {
    if (!downloading) return;
    const timer = setInterval(() => void refreshGameUpdates(), 1500);
    return () => clearInterval(timer);
  }, [downloading, refreshGameUpdates]);

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
  // A finished install leaves the list.
  useEffect(() => on<DirectDone>("direct://done", (d) => {
    if (d.ok) void load(true);
  }), [load]);

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
      if (useApp.getState().gameUpdates[item.id]) {
        setStopping(item);
      } else if (item.source === "deal") {
        void api.openLink(`steam://store/${item.id.slice("deal:".length)}`);
        toast(`${item.name} a ${item.price} nel negozio Steam. Premi Guide per tornare qui`);
      } else if (item.source === "gamepass" && item.sizeBytes) {
        // The Xbox app picks the drive itself: a heads-up, not a wall.
        void api
          .storageOverview()
          .then((o) => o.drives.find((d) => d.mount === o.xboxMount) ?? null)
          .catch(() => null)
          .then((drive) => {
            if (drive && drive.available < item.sizeBytes!) setTight({ item, drive });
            else void installFromCatalog(item.id, item.name);
          });
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
            {describe(focused, gameUpdates[focused.id])}
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
            <CatalogCard key={item.id} item={item} autofocus={i === 0} onFocus={onFocus} onSelect={onSelect} download={gameUpdates[item.id]} />
          ))}
        </div>
      )}
      <AnimatePresence>
        {stopping && (
          <ConfirmDialog
            key="stop"
            title={`Annullare il download di ${stopping.name}?`}
            text="I file scaricati finora vengono eliminati."
            confirmLabel="Annulla download"
            onCancel={() => setStopping(null)}
            onConfirm={() => {
              const id = stopping.id;
              setStopping(null);
              void api.cancelDirectInstall(id).then(() => refreshGameUpdates());
            }}
          />
        )}
        {tight && (
          <ConfirmDialog
            key="tight"
            title={`Spazio insufficiente su ${tight.drive.mount.replace(/[\\/]+$/, "")}`}
            text={`${tight.item.name} scarica ${formatBytes(tight.item.sizeBytes ?? 0)} e sull'unità ne restano ${formatBytes(tight.drive.available)}. Puoi liberare spazio da Impostazioni → Archiviazione, oppure scegliere un'altra unità nell'app Xbox.`}
            confirmLabel="Installa comunque"
            onCancel={() => setTight(null)}
            onConfirm={() => {
              setTight(null);
              void installFromCatalog(tight.item.id, tight.item.name);
            }}
          />
        )}
      </AnimatePresence>
    </div>
  );
}

function describe(item: CatalogItem, download?: GameUpdate) {
  if (download) {
    const progress = updateProgress(download);
    return download.state === "installing" ? "Installazione in corso" : `Download in corso${progress ? ` · ${progress}` : ""}`;
  }
  switch (item.source) {
    case "gamepass":
      return item.sizeBytes ? `Incluso in PC Game Pass · ${formatBytes(item.sizeBytes)}` : "Incluso in PC Game Pass";
    case "cloud":
      return "Gioca in streaming con Xbox Cloud Gaming";
    case "deal":
      return `In offerta: ${item.initial} → ${item.price}`;
    default:
      return item.playtimeSecs > 0 ? `Giocato ${formatPlaytime(item.playtimeSecs)}` : "Mai giocato";
  }
}
