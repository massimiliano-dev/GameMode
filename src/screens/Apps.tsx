import { useCallback, useMemo, useRef, useState } from "react";

import { AppCard } from "../components/AppCard";
import { Glyph, useHints, useScreenFocus } from "../components/common";
import { useAction } from "../input/dispatcher";
import { ensureFocus, forgetGroup } from "../input/focus";
import type { Game } from "../lib/types";
import { isApp, useApp, visibleGames } from "../store/app";

type Filter = "all" | "favorites" | "recent";
const FILTERS: { key: Filter; label: string }[] = [
  { key: "all", label: "Tutte" },
  { key: "favorites", label: "Preferite" },
  { key: "recent", label: "Usate di recente" },
];

const byName = (a: Game, b: Game) => a.name.localeCompare(b.name, "it");
const byRecent = (a: Game, b: Game) => (b.lastPlayed ?? 0) - (a.lastPlayed ?? 0) || byName(a, b);

/** Lowercase, accents stripped, so "calcolatrice" finds "Calcolatrice". */
function normalize(s: string) {
  return s.normalize("NFD").replace(/\p{Diacritic}/gu, "").toLowerCase();
}

/** Every installed application as a tile; A opens it right away. */
export function Apps({ focusId }: { focusId?: string }) {
  const all = useApp((s) => s.games);
  const loaded = useApp((s) => s.loaded);
  const scanning = useApp((s) => s.scanning);
  const settings = useApp((s) => s.settings);
  const open = useApp((s) => s.open);
  const launch = useApp((s) => s.launch);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const sessionGame = useApp((s) => s.session?.gameId);
  const [filter, setFilter] = useState<Filter>("all");
  const [query, setQuery] = useState("");
  const focused = useRef<Game | null>(null);

  const apps = useMemo(() => visibleGames(all).filter(isApp), [all]);
  const shown = useMemo(() => {
    const words = normalize(query).split(/\s+/).filter(Boolean);
    const list = apps.filter((a) => {
      if (words.length && !words.every((w) => normalize(a.name).includes(w))) return false;
      if (filter === "favorites") return a.favorite;
      if (filter === "recent") return !!a.lastPlayed;
      return true;
    });
    return list.sort(filter === "recent" ? byRecent : byName);
  }, [apps, filter, query]);
  const disabled = !!settings?.disabledStores.includes("app");

  useScreenFocus(focusId, apps.length > 0);
  useHints([
    { button: "A", label: "Apri" },
    { button: "X", label: "Dettagli" },
    { button: "Y", label: "Preferita" },
    { button: "LT", alt: "RT", label: "Filtro" },
  ]);

  const regroup = () => {
    // The grid content changes: start again from its first tile.
    forgetGroup("apps");
    requestAnimationFrame(() => {
      const first = document.querySelector<HTMLElement>(".apps-grid [data-nav]");
      first?.focus({ preventScroll: true });
      first?.scrollIntoView({ block: "nearest" });
      ensureFocus();
    });
  };

  useAction((action, repeat) => {
    if (repeat) return;
    if (action === "lt" || action === "rt") {
      const i = FILTERS.findIndex((f) => f.key === filter);
      setFilter(FILTERS[(i + (action === "rt" ? 1 : FILTERS.length - 1)) % FILTERS.length].key);
      regroup();
      return true;
    }
    if (action === "x" && focused.current) {
      open({ name: "detail", gameId: focused.current.id });
      return true;
    }
    if (action === "y" && focused.current) {
      void toggleFavorite(focused.current);
      return true;
    }
  });

  const onFocus = useCallback(
    (a: Game) => {
      focused.current = a;
      setBackdrop(a.id);
    },
    [setBackdrop],
  );
  const onSelect = useCallback((a: Game) => void launch(a.id), [launch]);

  if (!apps.length) return <EmptyApps loaded={loaded} scanning={scanning} disabled={disabled} />;

  return (
    <div className="screen apps">
      <header className="library-head">
        <h1 className="screen-title">
          App <span className="row-count">{shown.length}</span>
        </h1>
        <input
          className="text-input apps-search"
          data-nav
          data-nav-id="apps-search"
          aria-label="Cerca un'app"
          placeholder="Cerca un'app…"
          value={query}
          spellCheck={false}
          onChange={(e) => {
            setQuery(e.target.value);
            forgetGroup("apps");
          }}
        />
        <div className="chips">
          <Glyph button="LT" />
          {FILTERS.map((f) => (
            <span key={f.key} className={`chip ${f.key === filter ? "is-active" : ""}`}>
              {f.label}
            </span>
          ))}
          <Glyph button="RT" />
        </div>
      </header>
      {shown.length === 0 ? (
        <p className="empty-text library-empty">Nessuna app corrisponde.</p>
      ) : (
        <div className="apps-grid" data-nav-group="apps">
          {shown.map((a, i) => (
            <AppCard
              key={a.id}
              app={a}
              navId={`apps:${a.id}`}
              onFocus={onFocus}
              onSelect={onSelect}
              running={sessionGame === a.id}
              autofocus={i === 0}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function EmptyApps({ loaded, scanning, disabled }: { loaded: boolean; scanning: boolean; disabled: boolean }) {
  const rescan = useApp((s) => s.rescan);
  const setTab = useApp((s) => s.setTab);
  useScreenFocus(undefined, loaded);
  useHints([{ button: "A", label: "Seleziona" }]);

  if (!loaded || scanning) {
    return (
      <div className="screen empty">
        <div className="spinner" />
        <p className="empty-title">Cerco le app installate…</p>
      </div>
    );
  }
  return (
    <div className="screen empty">
      <p className="empty-title">{disabled ? "Le app sono disattivate" : "Nessuna app trovata"}</p>
      <p className="empty-text">
        {disabled
          ? "Attiva le app in Impostazioni → Librerie per vederle qui."
          : "Qui compaiono le app di Windows e del Microsoft Store: prova a cercarle di nuovo."}
      </p>
      <div className="button-row">
        {disabled ? (
          <button type="button" className="btn btn-primary" data-nav data-nav-autofocus onClick={() => setTab("settings")}>
            Impostazioni
          </button>
        ) : (
          <button type="button" className="btn btn-primary" data-nav data-nav-autofocus onClick={() => void rescan()}>
            Cerca di nuovo
          </button>
        )}
      </div>
    </div>
  );
}
