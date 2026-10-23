import { AnimatePresence } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { ActionSheet } from "../components/ActionSheet";
import { CatalogCard } from "../components/CatalogCard";
import { ConfirmDialog } from "../components/ConfirmDialog";

import { Glyph, StoreBadge, useHints, useScreenFocus } from "../components/common";
import { useAction } from "../input/dispatcher";
import { forgetGroup } from "../input/focus";
import { api, on, pickFolder } from "../lib/api";
import { formatBytes, formatPlaytime } from "../lib/format";
import { describeJob, jobActive } from "../lib/legendary";
import type { Catalog, CatalogItem, Deals, Drive, InstallTarget, LegendaryJob, LegendaryStatus } from "../lib/types";
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
  /** Epic through legendary: its state, its operations, and an install waiting for a folder. */
  const [legendary, setLegendary] = useState<LegendaryStatus | null>(null);
  const [jobs, setJobs] = useState<LegendaryJob[]>([]);
  const [epicInstall, setEpicInstall] = useState<{ item: CatalogItem; targets: InstallTarget[] } | null>(null);
  const legendaryOn = !!legendary?.enabled && legendary.downloaded;

  useEffect(() => {
    const status = () => void api.legendaryStatus().then(setLegendary).catch(() => {});
    status();
    void api.legendaryJobs().then(setJobs).catch(() => {});
    const offs = [on<LegendaryJob[]>("legendary://jobs", setJobs), on("legendary://changed", status)];
    return () => offs.forEach((off) => off());
  }, []);

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
  // Epic games legendary is installing (or just installed) leave the list.
  const takenKey = jobs
    .filter((j) => j.kind === "install" && (jobActive(j) || j.state === "done"))
    .map((j) => j.appName.toLowerCase())
    .sort()
    .join("\n");
  const takenApps = useMemo(() => new Set(takenKey.split("\n")), [takenKey]);
  const shown = useMemo(
    () =>
      items.filter(
        (i) =>
          (filter === "all" ? IN_ALL.has(i.source) : i.source === filter) &&
          !(i.source === "epic" && takenApps.has(epicAppName(i).toLowerCase())),
      ),
    [items, filter, takenApps],
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
      } else if (item.source === "epic" && legendaryOn) {
        if (!legendary?.account) {
          toast("Accedi al tuo account Epic in Impostazioni → Librerie", "error");
          return;
        }
        void api
          .legendaryInstallTargets()
          .then((targets) => setEpicInstall({ item, targets }))
          .catch((e) => toast(String(e), "error"));
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
    [installFromCatalog, toast, legendaryOn, legendary],
  );

  const installEpic = useCallback(
    async (item: CatalogItem, path: string) => {
      try {
        await api.legendaryInstall(epicAppName(item), path);
        toast(`Download di ${item.name} avviato`);
      } catch (e) {
        toast(String(e), "error");
      }
    },
    [toast],
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
            {describe(focused, legendaryOn)}
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

      {jobs.length > 0 && (
        <section className="legendary-jobs" data-nav-group="legendary-jobs" aria-label="Operazioni sui giochi Epic">
          {jobs.map((job) => (
            <div key={job.id} className={`legendary-job is-${job.state}`}>
              <div className="legendary-job-text">
                <span className="legendary-job-title">{job.title}</span>
                <span className="legendary-job-status">{describeJob(job)}</span>
              </div>
              {jobActive(job) && job.progress != null && (
                <span className="meter" role="img" aria-label={`${Math.round(job.progress * 100)}%`}>
                  <span className="meter-fill" style={{ width: `${Math.round(job.progress * 100)}%` }} />
                </span>
              )}
              {jobActive(job) && job.kind !== "uninstall" && job.kind !== "import" && (
                <button type="button" className="btn" data-nav onClick={() => void api.legendaryCancel(job.id)}>
                  {job.state === "queued" ? "Togli dalla coda" : "Annulla"}
                </button>
              )}
            </div>
          ))}
        </section>
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
      <AnimatePresence>
        {epicInstall && (
          <ActionSheet
            key="epic-install"
            title={`Dove installare ${epicInstall.item.name}?`}
            actions={[
              ...epicInstall.targets.map((t) => ({
                label: t.path,
                hint: [t.default ? "Consigliata" : null, t.free > 0 ? `${formatBytes(t.free)} liberi` : null].filter(Boolean).join(" · ") || undefined,
                onSelect: () => void installEpic(epicInstall.item, t.path),
              })),
              {
                label: "Scegli un'altra cartella…",
                onSelect: () =>
                  void pickFolder().then((path) => {
                    if (path) void installEpic(epicInstall.item, path);
                  }),
              },
            ]}
            onClose={() => setEpicInstall(null)}
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

/** `epic:<namespace>:<catalog id>:<app name>` → the app name legendary uses. */
function epicAppName(item: CatalogItem) {
  return item.id.split(":").slice(3).join(":");
}

function describe(item: CatalogItem, legendaryOn: boolean) {
  switch (item.source) {
    case "epic":
      return legendaryOn ? "Si installa senza Epic Games Launcher" : "Mai giocato";
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
