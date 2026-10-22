import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";

import { ActionSheet, type SheetAction } from "../components/ActionSheet";
import { CollectionsDialog } from "../components/CollectionsDialog";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { SavesDialog } from "../components/SavesDialog";
import { Cover } from "../components/GameCard";
import { SteamExtras } from "../components/SteamExtras";
import { StoreBadge, useHints, useScreenFocus } from "../components/common";
import { useAction } from "../input/dispatcher";
import { api, artUrl } from "../lib/api";
import { formatLastPlayed, formatPlaytime, SHOP_LABEL } from "../lib/format";
import type { Game } from "../lib/types";
import { useApp } from "../store/app";

type Dialog = "options" | "collections" | "saves" | "uninstall" | null;

export function GameDetail({ gameId }: { gameId: string }) {
  const game = useApp((s) => s.games.find((g) => g.id === gameId));
  const session = useApp((s) => s.session);
  const collections = useApp((s) => s.collections);
  const launch = useApp((s) => s.launch);
  const toggleFavorite = useApp((s) => s.toggleFavorite);
  const toggleHidden = useApp((s) => s.toggleHidden);
  const openStore = useApp((s) => s.openStore);
  const uninstall = useApp((s) => s.uninstall);
  const setBackdrop = useApp((s) => s.setBackdrop);
  const back = useApp((s) => s.back);
  const [dialog, setDialog] = useState<Dialog>(null);

  useEffect(() => setBackdrop(gameId), [gameId, setBackdrop]);
  useScreenFocus(undefined, !!game);
  useHints([
    { button: "A", label: "Seleziona" },
    { button: "menu", label: "Opzioni" },
    { button: "B", label: "Indietro" },
  ]);
  useAction((action, repeat) => {
    if (action === "menu" && !repeat && game && !dialog) {
      setDialog("options");
      return true;
    }
  });

  if (!game) {
    return (
      <div className="screen empty">
        <p className="empty-title">Gioco non più disponibile</p>
        <button type="button" className="btn" data-nav data-nav-autofocus onClick={back}>
          Indietro
        </button>
      </div>
    );
  }

  const logo = artUrl(game.artwork.logo, game.artworkCheckedAt);
  const mine = session?.gameId === game.id ? session : null;
  const inCollections = collections.filter((c) => c.gameIds.includes(game.id));

  const options: SheetAction[] = [
    ...(SHOP_LABEL[game.store]
      ? [{ label: "Pagina nello store", hint: "DLC, espansioni e aggiornamenti", onSelect: () => void openStore(game.store, game.id) }]
      : []),
    ...(game.store !== "mock"
      ? [{ label: "Salvataggi", hint: "Backup e ripristino", onSelect: () => setDialog("saves") }]
      : []),
    {
      label: game.hidden ? "Mostra nella libreria" : "Nascondi",
      hint: game.hidden ? undefined : "Lo ritrovi in Impostazioni → Giochi nascosti",
      onSelect: () => {
        void toggleHidden(game);
        if (!game.hidden) back();
      },
    },
    ...(game.store !== "mock"
      ? [{ label: game.store === "manual" ? "Rimuovi dalla libreria" : "Disinstalla", danger: true, onSelect: () => setDialog("uninstall") }]
      : []),
  ];

  return (
    <div className="screen detail">
      <div className="detail-scroll">
        <div className="detail-top">
          <div className="detail-main">
            {logo ? <img className="detail-logo" src={logo} alt={game.name} /> : <h1 className="detail-title">{game.name}</h1>}
            <div className="featured-meta">
              <StoreBadge store={game.store} />
              {game.favorite && <span className="fav-star">★ Preferito</span>}
              {inCollections.map((c) => (
                <span key={c.id} className="chip">
                  {c.name}
                </span>
              ))}
            </div>
    
            <div className="button-row detail-actions" data-nav-group="actions">
              {mine ? (
                <>
                  <button type="button" className="btn btn-primary btn-play" data-nav data-nav-autofocus onClick={() => void api.resumeGame()}>
                    {mine.phase === "launching" ? "Avvio in corso…" : "Torna al gioco"}
                  </button>
                  <button type="button" className="btn" data-nav onClick={() => void api.closeGame()}>
                    Chiudi gioco
                  </button>
                </>
              ) : (
                <button type="button" className="btn btn-primary btn-play" data-nav data-nav-autofocus onClick={() => void launch(game.id)}>
                  <span className="play-icon" aria-hidden>▶</span> Gioca
                </button>
              )}
              <button type="button" className="btn" data-nav onClick={() => void toggleFavorite(game)}>
                {game.favorite ? "★ Preferito" : "☆ Preferito"}
              </button>
              <button type="button" className="btn" data-nav onClick={() => setDialog("collections")}>
                Raccolte
              </button>
              <button type="button" className="btn" data-nav aria-label="Altre opzioni" onClick={() => setDialog("options")}>
                ··· Opzioni
              </button>
            </div>
    
            <dl className="stats">
              <div className="stat">
                <dt>Tempo di gioco</dt>
                <dd>{formatPlaytime(game.playtimeSecs)}</dd>
              </div>
              <div className="stat">
                <dt>Ultima partita</dt>
                <dd>{formatLastPlayed(game.lastPlayed)}</dd>
              </div>
              {game.installDir && (
                <div className="stat stat-wide">
                  <dt>Installato in</dt>
                  <dd className="mono">{game.installDir}</dd>
                </div>
              )}
            </dl>
          </div>
          <div className="detail-cover">
            <Cover game={game} />
          </div>
        </div>
        {game.steamAppid && game.store === "steam" && <SteamExtras appid={game.steamAppid} />}
      </div>
      <AnimatePresence>
        {dialog === "options" && <ActionSheet key="options" title={game.name} actions={options} onClose={() => setDialog(null)} />}
        {dialog === "collections" && <CollectionsDialog key="collections" game={game} onClose={() => setDialog(null)} />}
        {dialog === "saves" && <SavesDialog key="saves" game={game} onClose={() => setDialog(null)} />}
        {dialog === "uninstall" && (
          <ConfirmDialog
            key="uninstall"
            title={game.store === "manual" ? `Rimuovere ${game.name}?` : `Disinstallare ${game.name}?`}
            text={uninstallText(game)}
            confirmLabel={game.store === "manual" ? "Rimuovi" : "Disinstalla"}
            onCancel={() => setDialog(null)}
            onConfirm={() => {
              setDialog(null);
              void uninstall(game);
            }}
          />
        )}
      </AnimatePresence>
    </div>
  );
}

function uninstallText(game: Game) {
  if (game.store === "manual") return "Il gioco sparisce da GameMode, i suoi file restano dove sono.";
  if (game.store === "xbox") return "Il gioco viene rimosso dal PC. I salvataggi nel cloud restano sul tuo account Xbox.";
  return "Si apre la disinstallazione dello store, che potrebbe chiederti un'ulteriore conferma.";
}
