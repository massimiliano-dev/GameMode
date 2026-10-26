import { AnimatePresence } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { ActionSheet } from "../components/ActionSheet";
import { AppCard } from "../components/AppCard";
import { CaptureRow } from "../components/CaptureGallery";
import { Cover, GameCard } from "../components/GameCard";
import { StoreBadge, useHints, useScreenFocus } from "../components/common";
import { SystemUpdateRow, SystemUpdateSheet } from "../components/SystemUpdateCard";
import { useAction } from "../input/dispatcher";
import { ensureFocus, focusElement } from "../input/focus";
import { artUrl } from "../lib/api";
import { formatLastPlayed, formatPlaytime, STORE_LABEL, STORES } from "../lib/format";
import type { Collection, Game, SessionState, Store } from "../lib/types";
import { isApp, libraryGames, useApp, visibleGames } from "../store/app";

interface Row {
  key: string;
  title: string;
  games: Game[];
}

const ROW_LIMIT = 14;
/** The row of app tiles, where A opens the app. */
const APPS_ROW = "apps";

function buildRows(all: Game[], collections: Collection[]): Row[] {
  const games = libraryGames(visibleGames(all));
  const rows: Row[] = [];
  const recent = games
    .filter((g) => g.lastPlayed)
    .sort((a, b) => (b.lastPlayed ?? 0) - (a.lastPlayed ?? 0))
    .slice(0, ROW_LIMIT);
  if (recent.length) rows.push({ key: "recent", title: "Continua a giocare", games: recent });

  const favorites = games.filter((g) => g.favorite);
  if (favorites.length) rows.push({ key: "favorites", title: "Preferiti", games: favorites });

  // Apps opened from GameMode (Netflix, Spotify…), as tiles.
  const apps = visibleGames(all)
    .filter((g) => isApp(g) && g.lastPlayed)
    .sort((a, b) => (b.lastPlayed ?? 0) - (a.lastPlayed ?? 0))
    .slice(0, ROW_LIMIT);
  if (apps.length) rows.push({ key: APPS_ROW, title: "App usate di recente", games: apps });

  for (const c of collections) {
    const list = c.gameIds.map((id) => games.find((g) => g.id === id)).filter((g): g is Game => !!g);
    if (list.length) rows.push({ key: `c-${c.id}`, title: c.name, games: list });
  }

  // On first launch every game shares the same timestamp: nothing is "new".
  const oldest = Math.min(...games.map((g) => g.addedAt));
  const added = games
    .filter((g) => g.addedAt > oldest + 3600)
    .sort((a, b) => b.addedAt - a.addedAt)
    .slice(0, ROW_LIMIT);
  if (added.length) rows.push({ key: "added", title: "Aggiunti di recente", games: added });

  for (const store of STORES) {
    const list = games.filter((g) => g.store === store);
    if (list.length) rows.push({ key: store, title: STORE_LABEL[store], games: list });
  }
  if (!rows.length && games.length) rows.push({ key: "all", title: "Tutti i giochi", games });
  return rows;
}

export function Home({ focusId }: { focusId?: string }) {
  const games = useApp((s) => s.games);
  const collections = useApp((s) => s.collections);
  const loaded = useApp((s) => s.loaded);
  const open = useApp((s) => s.open);
  const launch = useApp((s) => s.launch);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const featured = useApp((s) => s.games.find((g) => g.id === s.backdropId));
  const sessionGame = useApp((s) => s.session?.gameId);
  const suspended = useApp((s) => s.suspended);
  const resumeSuspended = useApp((s) => s.resumeSuspended);
  const closeSuspended = useApp((s) => s.closeSuspended);
  // The PC just woke up with a game still running.
  const wokeGame = useApp((s) => (s.wokeAt && s.session ? s.games.find((g) => g.id === s.session?.gameId) : undefined));
  const resumeAfterWake = useApp((s) => s.resumeAfterWake);
  const rows = useMemo(() => buildRows(games, collections), [games, collections]);
  // Frozen games (Quick Resume), with their sessions.
  const paused = useMemo(() => pausedGames(games, suspended), [games, suspended]);
  const [sheet, setSheet] = useState<Game | null>(null);
  // The focus sits on the "In pausa" row, where A opens the options.
  const [onPausedRow, setOnPausedRow] = useState(false);
  // …or on the apps row, where A opens the app.
  const [onAppsRow, setOnAppsRow] = useState(false);
  // The focus sits on the "Bentornato" card: X resumes the game left open.
  const [onWakeRow, setOnWakeRow] = useState(false);
  const wakeResume = useRef<HTMLButtonElement>(null);
  const wokeId = wokeGame?.id;
  // The focus sits on the Windows update card: no game to play there.
  const [onSystemCard, setOnSystemCard] = useState(false);
  const [systemSheet, setSystemSheet] = useState(false);
  // The focus sits on "Le tue catture": A opens the picture, X/Y do nothing.
  const [onCaptureRow, setOnCaptureRow] = useState(false);

  useScreenFocus(focusId, rows.length > 0 || paused.length > 0);
  useHints(
    onSystemCard
      ? [{ button: "A", label: "Opzioni" }]
      : onCaptureRow
        ? [{ button: "A", label: "Visualizza" }]
        : onWakeRow
          ? [
              { button: "A", label: "Seleziona" },
              { button: "X", label: "Riprendi" },
            ]
          : [
              { button: "A", label: onPausedRow ? "Opzioni" : onAppsRow ? "Apri" : "Dettagli" },
              { button: "X", label: onAppsRow ? "Dettagli" : "Gioca" },
              { button: "Y", label: onAppsRow ? "Preferita" : "Preferito" },
            ],
    [onPausedRow, onAppsRow, onWakeRow, onSystemCard, onCaptureRow],
  );

  // The card comes and goes while Home is up (a wake, the game ending):
  // the focus goes to "Riprendi la partita", and back to the rows after.
  const shownWake = useRef(wokeId);
  useEffect(() => {
    // On mount the screen's own focus (useScreenFocus) stands.
    if (shownWake.current === wokeId) return;
    shownWake.current = wokeId;
    requestAnimationFrame(() => {
      if (wokeId && wakeResume.current?.isConnected) focusElement(wakeResume.current);
      else ensureFocus();
    });
  }, [wokeId]);
  const onFocusWake = useCallback(() => {
    if (wokeId) setBackdrop(wokeId);
    setOnWakeRow(true);
    setOnPausedRow(false);
    setOnAppsRow(false);
    setOnSystemCard(false);
    setOnCaptureRow(false);
  }, [wokeId, setBackdrop]);

  const onFocus = useCallback(
    (g: Game) => {
      setBackdrop(g.id);
      setOnPausedRow(false);
      setOnAppsRow(false);
      setOnWakeRow(false);
      setOnSystemCard(false);
      setOnCaptureRow(false);
    },
    [setBackdrop],
  );
  const onFocusPaused = useCallback(
    (g: Game) => {
      setBackdrop(g.id);
      setOnPausedRow(true);
      setOnAppsRow(false);
      setOnWakeRow(false);
      setOnSystemCard(false);
      setOnCaptureRow(false);
    },
    [setBackdrop],
  );
  const onFocusApp = useCallback(
    (g: Game) => {
      setBackdrop(g.id);
      setOnPausedRow(false);
      setOnAppsRow(true);
      setOnWakeRow(false);
      setOnSystemCard(false);
      setOnCaptureRow(false);
    },
    [setBackdrop],
  );
  const onFocusSystem = useCallback(() => {
    setOnSystemCard(true);
    setOnPausedRow(false);
    setOnAppsRow(false);
    setOnWakeRow(false);
    setOnCaptureRow(false);
  }, []);
  const onFocusCapture = useCallback(() => {
    setOnCaptureRow(true);
    setOnPausedRow(false);
    setOnAppsRow(false);
    setOnWakeRow(false);
    setOnSystemCard(false);
  }, []);
  const onSelect = useCallback((g: Game) => open({ name: "detail", gameId: g.id }), [open]);
  const onSelectPaused = useCallback((g: Game) => setSheet(g), []);
  const onSelectApp = useCallback((g: Game) => void launch(g.id), [launch]);
  const sheetKnown = !!sheet && games.some((g) => g.id === sheet.id);

  useAction((action, repeat) => {
    if (repeat || !featured || sheet || onSystemCard || onCaptureRow) return;
    if (onWakeRow && wokeGame) {
      // The buttons speak for the game left open, not for the last card.
      if (action === "x") {
        void resumeAfterWake();
        return true;
      }
      if (action === "y") return true;
    }
    if (action === "x") {
      if (onAppsRow) open({ name: "detail", gameId: featured.id });
      else void launch(featured.id);
      return true;
    }
    if (action === "y") {
      void toggleFavorite(featured);
      return true;
    }
  });

  if (!rows.length && !paused.length) return <EmptyLibrary loaded={loaded} />;

  return (
    <div className="screen home">
      <section className="home-hero">{featured && <FeaturedInfo game={featured} />}</section>
      <div className="rows">
        {wokeGame && (
          <section className="row wake-row" data-nav-group="row:wake">
            <h2 className="row-title">Bentornato</h2>
            <div className="wake-card">
              <div className="wake-cover">
                <Cover game={wokeGame} />
              </div>
              <div className="wake-info">
                <span className="wake-title">{wokeGame.name}</span>
                <span className="wake-text">La partita è rimasta aperta durante la sospensione</span>
              </div>
              <div className="wake-actions">
                <button
                  ref={wakeResume}
                  type="button"
                  className="btn btn-primary"
                  data-nav
                  data-nav-autofocus
                  onFocus={onFocusWake}
                  onClick={() => void resumeAfterWake()}
                >
                  Riprendi la partita
                </button>
                <button type="button" className="btn" data-nav onFocus={onFocusWake} onClick={() => useApp.setState({ wokeAt: null })}>
                  Resta qui
                </button>
              </div>
            </div>
          </section>
        )}
        <SystemUpdateRow onFocus={onFocusSystem} onSelect={() => setSystemSheet(true)} />
        {paused.length > 0 && (
          <section className="row" data-nav-group="row:paused">
            <h2 className="row-title">
              In pausa
              <span className="row-count">{paused.length}</span>
            </h2>
            <div className="row-scroll">
              {paused.map(({ game }, i) => (
                <GameCard
                  key={game.id}
                  game={game}
                  navId={`paused:${game.id}`}
                  onFocus={onFocusPaused}
                  onSelect={onSelectPaused}
                  paused
                  autofocus={!wokeGame && i === 0}
                />
              ))}
            </div>
          </section>
        )}
        {rows.map((row, rowIndex) => (
          <section key={row.key} className="row" data-nav-group={`row:${row.key}`}>
            <h2 className="row-title">
              {row.title}
              <span className="row-count">{row.games.length}</span>
            </h2>
            <div className="row-scroll">
              {row.games.map((g, i) =>
                row.key === APPS_ROW ? (
                  <AppCard
                    key={g.id}
                    app={g}
                    navId={`${row.key}:${g.id}`}
                    onFocus={onFocusApp}
                    onSelect={onSelectApp}
                    running={sessionGame === g.id}
                    autofocus={!wokeGame && !paused.length && rowIndex === 0 && i === 0}
                  />
                ) : (
                  <GameCard
                    key={g.id}
                    game={g}
                    navId={`${row.key}:${g.id}`}
                    onFocus={onFocus}
                    onSelect={onSelect}
                    playing={sessionGame === g.id}
                    paused={suspended.some((p) => p.gameId === g.id)}
                    autofocus={!wokeGame && !paused.length && rowIndex === 0 && i === 0}
                  />
                ),
              )}
            </div>
          </section>
        ))}
        <CaptureRow onFocus={onFocusCapture} />
      </div>
      <AnimatePresence>
        {sheet && (
          <ActionSheet
            key="paused"
            title={sheet.name}
            onClose={() => setSheet(null)}
            actions={[
              { label: "Riprendi", hint: pausedFor(paused.find((p) => p.game.id === sheet.id)?.session), onSelect: () => void resumeSuspended(sheet.id) },
              // A game the library lost (store disabled, rescan) has no page to open.
              ...(sheetKnown ? [{ label: "Dettagli", onSelect: () => onSelect(sheet) }] : []),
              { label: "Chiudi gioco", danger: true, onSelect: () => void closeSuspended(sheet.id) },
            ]}
          />
        )}
      </AnimatePresence>
      <SystemUpdateSheet open={systemSheet} onClose={() => setSystemSheet(false)} />
    </div>
  );
}

/** The frozen games, oldest pause first. One the library no longer lists
 * (store disabled, rescan) still shows, so it can be resumed or closed. */
function pausedGames(games: Game[], suspended: SessionState[]): { game: Game; session: SessionState }[] {
  return suspended.map((session) => ({ session, game: games.find((g) => g.id === session.gameId) ?? placeholderGame(session) }));
}

function placeholderGame(session: SessionState): Game {
  const prefix = session.gameId.split(":")[0];
  const store = (STORES as string[]).includes(prefix) || prefix === "app" ? (prefix as Store) : "manual";
  return {
    id: session.gameId,
    store,
    name: session.gameName || session.gameId,
    installDir: null,
    steamAppid: null,
    artwork: {},
    artworkCheckedAt: null,
    favorite: false,
    hidden: false,
    addedAt: 0,
    lastPlayed: null,
    playtimeSecs: 0,
    sizeBytes: null,
    sizeCheckedAt: null,
    sizePartial: false,
  };
}

function pausedFor(session?: SessionState): string | undefined {
  if (!session?.suspendedAt) return undefined;
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - session.suspendedAt);
  return secs < 60 ? "In pausa da meno di un minuto" : `In pausa da ${formatPlaytime(secs)}`;
}

function FeaturedInfo({ game }: { game: Game }) {
  const logo = artUrl(game.artwork.logo, game.artworkCheckedAt);
  return (
    <div className="featured" key={game.id}>
      {logo ? (
        <img className="featured-logo" src={logo} alt={game.name} />
      ) : (
        <h1 className="featured-title">{game.name}</h1>
      )}
      <div className="featured-meta">
        <StoreBadge store={game.store} />
        <span>{formatPlaytime(game.playtimeSecs)}</span>
        <span className="sep" />
        <span>{game.store === "app" ? "Ultimo utilizzo" : "Ultima partita"}: {formatLastPlayed(game.lastPlayed)}</span>
      </div>
    </div>
  );
}

function EmptyLibrary({ loaded }: { loaded: boolean }) {
  const rescan = useApp((s) => s.rescan);
  const scanning = useApp((s) => s.scanning);
  const setTab = useApp((s) => s.setTab);
  useScreenFocus(undefined, loaded);
  useHints([{ button: "A", label: "Seleziona" }]);

  if (!loaded || scanning) {
    return (
      <div className="screen empty">
        <div className="spinner" />
        <p className="empty-title">Cerco i tuoi giochi…</p>
        <p className="empty-text">Steam, Xbox, Epic, GOG, EA e Ubisoft</p>
      </div>
    );
  }
  return (
    <div className="screen empty">
      <div className="empty-art">
        <Cover game={{ id: "x", name: "?", store: "manual", artwork: {} } as Game} />
      </div>
      <p className="empty-title">Nessun gioco trovato</p>
      <p className="empty-text">
        Installa un gioco da uno store, oppure aggiungi un eseguibile a mano dalle impostazioni.
      </p>
      <div className="button-row">
        <button type="button" className="btn btn-primary" data-nav data-nav-autofocus onClick={() => void rescan()}>
          Cerca di nuovo
        </button>
        <button type="button" className="btn" data-nav onClick={() => setTab("settings")}>
          Impostazioni
        </button>
      </div>
    </div>
  );
}
