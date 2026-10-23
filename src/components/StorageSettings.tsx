import { AnimatePresence } from "motion/react";
import { useCallback, useEffect, useMemo, useState } from "react";

import { ensureFocus } from "../input/focus";
import { api, on } from "../lib/api";
import { driveOf, formatBytes, formatLastPlayed, STORE_LABEL } from "../lib/format";
import type { Cleanup, Drive, Game, StorageOverview } from "../lib/types";
import { useApp } from "../store/app";
import { ActionSheet, type SheetAction } from "./ActionSheet";
import { ConfirmDialog } from "./ConfirmDialog";
import { Row, Section } from "./SettingsRows";

/** Games listed before "Mostra tutti". */
const SHORT_LIST = 25;

function driveName(d: Drive) {
  const mount = d.mount.replace(/[\\/]+$/, "") || d.mount;
  return d.label ? `${mount} · ${d.label}` : mount;
}

function cleanupHint(c: Cleanup | null) {
  if (!c) return "Copertine di giochi disinstallati e backup automatici più vecchi";
  const parts = [
    c.artworkItems ? `copertine di ${c.artworkItems} ${c.artworkItems === 1 ? "gioco disinstallato" : "giochi disinstallati"}` : null,
    c.backupItems ? `${c.backupItems} backup automatici vecchi` : null,
  ].filter(Boolean);
  const bytes = c.artworkBytes + c.backupBytes;
  return parts.length ? `Recuperabili ${formatBytes(bytes)}: ${parts.join(", ")}` : "Niente da pulire";
}

/** Space per drive, games by size (with uninstall) and cleanup of GameMode's own leftovers. */
export function StorageSettings() {
  const games = useApp((s) => s.games);
  const open = useApp((s) => s.open);
  const uninstall = useApp((s) => s.uninstall);
  const toast = useApp((s) => s.toast);
  const [overview, setOverview] = useState<StorageOverview | null>(null);
  const [cleanup, setCleanup] = useState<Cleanup | null>(null);
  const [cleaning, setCleaning] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const [picked, setPicked] = useState<Game | null>(null);
  const [dialog, setDialog] = useState<"options" | "uninstall" | "cleanup" | null>(null);

  const load = useCallback(() => {
    void api.storageOverview().then(setOverview).catch(() => setOverview(null));
    void api.storageCleanup(false).then(setCleanup).catch(() => setCleanup(null));
  }, []);

  useEffect(() => {
    load();
    return on("storage://measured", load);
  }, [load]);

  const bySize = useMemo(
    () =>
      games
        .filter((g) => g.store !== "mock" || g.sizeBytes != null)
        .sort((a, b) => (b.sizeBytes ?? -1) - (a.sizeBytes ?? -1) || a.name.localeCompare(b.name, "it")),
    [games],
  );
  const shown = showAll ? bySize : bySize.slice(0, SHORT_LIST);
  const unknown = overview?.unknown ?? 0;

  const runCleanup = async () => {
    setDialog(null);
    setCleaning(true);
    try {
      const done = await api.storageCleanup(true);
      toast(`Liberati ${formatBytes(done.artworkBytes + done.backupBytes)}`);
      load();
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setCleaning(false);
    }
  };

  const options: SheetAction[] = picked
    ? [
        { label: "Dettagli", onSelect: () => open({ name: "detail", gameId: picked.id }) },
        ...(picked.store !== "emulator" && picked.store !== "mock"
          ? [{ label: picked.store === "manual" ? "Rimuovi dalla libreria" : "Disinstalla", danger: true, onSelect: () => setDialog("uninstall") }]
          : []),
      ]
    : [];

  return (
    <>
      <Section title="Spazio su disco">
        {overview ? (
          overview.drives.map((d) => {
            const used = Math.max(0, d.total - d.available);
            const other = Math.max(0, used - d.gamesBytes);
            return (
              <div key={d.mount} className="setting-row is-static drive-row">
                <div className="drive-head">
                  <span className="setting-label">
                    {driveName(d)}
                    {d.removable && <span className="setting-hint"> · rimovibile</span>}
                  </span>
                  <span className="setting-hint">
                    {formatBytes(d.available)} liberi di {formatBytes(d.total)}
                  </span>
                </div>
                <span className="drive-track" aria-hidden>
                  <span className="drive-bar drive-bar-other" style={{ width: `${(other / d.total) * 100}%` }} />
                  <span className="drive-bar drive-bar-games" style={{ width: `${(d.gamesBytes / d.total) * 100}%` }} />
                </span>
                <span className="drive-legend">
                  <span>
                    <i className="drive-bar-games" /> Giochi {formatBytes(d.gamesBytes)}
                    {d.games ? ` (${d.games})` : ""}
                  </span>
                  <span>
                    <i className="drive-bar-other" /> Altro {formatBytes(other)}
                  </span>
                </span>
              </div>
            );
          })
        ) : (
          <p className="setting-hint settings-note">Leggo le unità…</p>
        )}
        {overview?.measuring ? (
          <p className="setting-hint settings-note">Sto misurando le cartelle dei giochi: i totali si aggiornano da soli.</p>
        ) : unknown > 0 ? (
          <p className="setting-hint settings-note">
            {unknown === 1 ? "Un gioco non ha" : `${unknown} giochi non hanno`} una dimensione nota: viene misurata quando nessun gioco è
            in corso.
          </p>
        ) : null}
      </Section>

      <Section title="Giochi per dimensione">
        {shown.map((g, i) => (
          <Row
            key={g.id}
            label={g.name}
            hint={[STORE_LABEL[g.store], driveOf(g.installDir), `ultima partita ${formatLastPlayed(g.lastPlayed)}`].filter(Boolean).join(" · ")}
            autofocus={i === 0}
            onClick={() => {
              setPicked(g);
              setDialog("options");
            }}
            control={
              <span className="storage-size">
                {g.sizeBytes == null ? "—" : `${g.sizePartial ? "~" : ""}${formatBytes(g.sizeBytes)}`}
              </span>
            }
          />
        ))}
        {bySize.length > SHORT_LIST && (
          <Row
            label={showAll ? "Mostra solo i più grandi" : `Mostra tutti (${bySize.length})`}
            onClick={() => {
              setShowAll(!showAll);
              requestAnimationFrame(ensureFocus);
            }}
          />
        )}
        {bySize.length === 0 && <p className="setting-hint settings-note">Nessun gioco installato.</p>}
      </Section>

      <Section title="Pulizia">
        <Row
          label={cleaning ? "Pulizia in corso…" : "Libera spazio"}
          hint={cleanupHint(cleanup)}
          onClick={() => {
            if (cleaning || !cleanup) return;
            if (cleanup.artworkItems + cleanup.backupItems === 0) return toast("Niente da pulire");
            setDialog("cleanup");
          }}
          control={cleanup && cleanup.artworkBytes + cleanup.backupBytes > 0 ? <span className="setting-action">Pulisci</span> : undefined}
        />
        <Row
          label="Misura di nuovo i giochi"
          hint="Ricalcola lo spazio occupato dalle cartelle dei giochi che non lo dichiarano (GOG, EA, Ubisoft, Xbox, aggiunti a mano)"
          onClick={() => {
            void api.storageRemeasure();
            toast("Misurazione avviata in background");
            setTimeout(load, 500);
          }}
        />
        <p className="setting-hint settings-note">
          La disinstallazione dagli store (Steam, Epic, GOG…) libera lo spazio quando il loro client ha finito: i totali si aggiornano alla
          prossima ricerca dei giochi.
        </p>
      </Section>

      <AnimatePresence>
        {dialog === "options" && picked && <ActionSheet key="options" title={picked.name} actions={options} onClose={() => setDialog(null)} />}
        {dialog === "uninstall" && picked && (
          <ConfirmDialog
            key="uninstall"
            title={picked.store === "manual" ? `Rimuovere ${picked.name}?` : `Disinstallare ${picked.name}?`}
            text={
              picked.store === "manual"
                ? "Il gioco sparisce da GameMode, i suoi file restano dove sono."
                : picked.store === "xbox"
                  ? "Il gioco viene rimosso dal PC. I salvataggi nel cloud restano sul tuo account Xbox."
                  : "Si apre la disinstallazione dello store, che potrebbe chiederti un'ulteriore conferma."
            }
            confirmLabel={picked.store === "manual" ? "Rimuovi" : "Disinstalla"}
            onCancel={() => setDialog(null)}
            onConfirm={() => {
              setDialog(null);
              void uninstall(picked, true).then(load);
              requestAnimationFrame(ensureFocus);
            }}
          />
        )}
        {dialog === "cleanup" && cleanup && (
          <ConfirmDialog
            key="cleanup"
            title={`Liberare ${formatBytes(cleanup.artworkBytes + cleanup.backupBytes)}?`}
            text={`${cleanupHint(cleanup)}. Le copertine si riscaricano se reinstalli il gioco; dei backup automatici restano gli ultimi 3 per gioco.`}
            confirmLabel="Pulisci"
            onCancel={() => setDialog(null)}
            onConfirm={() => void runCleanup()}
          />
        )}
      </AnimatePresence>
    </>
  );
}
