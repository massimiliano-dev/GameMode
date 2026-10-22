import { useEffect, useMemo, useState } from "react";

import { EmulatorSettings } from "../components/EmulatorSettings";
import { SystemSettings } from "../components/SystemSettings";
import { UpdateSettings } from "../components/UpdateSettings";
import { ProfileSettings } from "../components/ProfileSettings";
import { Row, Section } from "../components/SettingsRows";
import { Toggle, useHints, useScreenFocus } from "../components/common";
import { ensureFocus } from "../input/focus";
import { api, pickExecutable, pickFolder } from "../lib/api";
import { STORE_LABEL, STORES } from "../lib/format";
import type { ManualGame, Store } from "../lib/types";
import { useApp } from "../store/app";

interface Draft {
  path: string;
  name: string;
  args: string;
}

export function Settings() {
  const settings = useApp((s) => s.settings);
  const saveSettings = useApp((s) => s.saveSettings);
  const rescan = useApp((s) => s.rescan);
  const scanning = useApp((s) => s.scanning);
  const scanErrors = useApp((s) => s.scanErrors);
  const games = useApp((s) => s.games);
  const hidden = useMemo(() => games.filter((g) => g.hidden), [games]);
  const toggleHidden = useApp((s) => s.toggleHidden);
  const toast = useApp((s) => s.toast);
  const setBackdrop = useApp((s) => s.setBackdrop);

  const [autostart, setAutostart] = useState(false);
  const [manual, setManual] = useState<ManualGame[]>([]);
  const [key, setKey] = useState(settings?.steamgriddbKey ?? "");
  const [steamKey, setSteamKey] = useState(settings?.steamApiKey ?? "");
  const [draft, setDraft] = useState<Draft | null>(null);
  const [backingUp, setBackingUp] = useState(false);

  useEffect(() => {
    setBackdrop(null);
    void api.getAutostart().then(setAutostart);
    void api.getManualGames().then(setManual);
  }, [setBackdrop]);
  useScreenFocus(undefined, !!settings);
  useHints([
    { button: "A", label: "Seleziona" },
    { button: "B", label: "Indietro" },
  ]);

  if (!settings) return null;

  const toggleStore = async (store: Store) => {
    const disabled = settings.disabledStores.includes(store)
      ? settings.disabledStores.filter((s) => s !== store)
      : [...settings.disabledStores, store];
    await saveSettings({ ...settings, disabledStores: disabled });
    void rescan();
  };

  const saveKey = async () => {
    await saveSettings({ ...settings, steamgriddbKey: key.trim() || null });
    toast(key.trim() ? "Chiave salvata, scarico le copertine mancanti" : "Chiave rimossa");
  };

  const saveSteamKey = async () => {
    await saveSettings({ ...settings, steamApiKey: steamKey.trim() || null });
    toast(steamKey.trim() ? "Chiave Steam salvata: i giochi non installati compaiono in Scarica" : "Chiave Steam rimossa");
  };

  const startAdd = async () => {
    const path = await pickExecutable();
    if (!path) return;
    const file = path.split(/[\\/]/).pop() ?? path;
    setDraft({ path, name: file.replace(/\.(exe|bat|cmd|lnk)$/i, ""), args: "" });
    requestAnimationFrame(() => document.querySelector<HTMLElement>("[data-nav-id='draft-name']")?.focus());
  };

  const saveDraft = async () => {
    if (!draft || !draft.name.trim()) return;
    try {
      await api.addManualGame(draft.name, draft.path, splitArgs(draft.args));
      setManual(await api.getManualGames());
      setDraft(null);
      await rescan();
    } catch (e) {
      toast(String(e), "error");
    }
  };

  const removeManual = async (m: ManualGame) => {
    await api.removeManualGame(m.id);
    setManual(await api.getManualGames());
    requestAnimationFrame(ensureFocus);
    await rescan();
  };

  return (
    <div className="screen settings">
      <h1 className="screen-title">Impostazioni</h1>
      <div className="settings-scroll">
        <Section title="Generale">
          <Row
            label="Avvia con Windows"
            hint="GameMode si apre da solo all'accensione del PC"
            autofocus
            onClick={async () => {
              await api.setAutostart(!autostart).catch((e) => toast(String(e), "error"));
              setAutostart(await api.getAutostart());
            }}
            control={<Toggle on={autostart} />}
          />
          <Row
            label="Schermo intero all'avvio"
            hint="Premi F11 per alternare in qualsiasi momento"
            onClick={() => void saveSettings({ ...settings, startFullscreen: !settings.startFullscreen })}
            control={<Toggle on={settings.startFullscreen} />}
          />
          <Row
            label="Overlay durante il gioco"
            hint="Guide apre un pannello sopra il gioco invece di tornare a GameMode (giochi in finestra senza bordi)"
            onClick={() => void saveSettings({ ...settings, overlayEnabled: !settings.overlayEnabled })}
            control={<Toggle on={settings.overlayEnabled} />}
          />
          <Row
            label="Suoni dell'interfaccia"
            hint="Click leggeri quando ti muovi e selezioni"
            onClick={() => void saveSettings({ ...settings, uiSounds: !settings.uiSounds })}
            control={<Toggle on={settings.uiSounds} />}
          />
          <Row
            label="Avvia Steam in background"
            hint="Steam parte nascosto insieme a GameMode, così i giochi si avviano prima"
            onClick={() => void saveSettings({ ...settings, preloadSteam: !settings.preloadSteam })}
            control={<Toggle on={settings.preloadSteam} />}
          />
        </Section>

        <SystemSettings settings={settings} />

        <ProfileSettings />

        <Section title="Librerie">
          {STORES.filter((s) => s !== "manual").map((store) => (
            <Row
              key={store}
              label={STORE_LABEL[store]}
              hint={scanErrors.find((e) => e.store === store)?.message}
              onClick={() => void toggleStore(store)}
              control={<Toggle on={!settings.disabledStores.includes(store)} />}
            />
          ))}
          <Row
            label={scanning ? "Ricerca in corso…" : "Cerca di nuovo i giochi"}
            hint="Rileva giochi installati o disinstallati"
            onClick={() => void rescan()}
          />
        </Section>

        <Section title="Libreria Steam completa">
          <div className="setting-row is-static">
            <div className="setting-text">
              <span className="setting-label">Chiave Web API di Steam</span>
              <span className="setting-hint">
                Serve per mostrare in Scarica i giochi Steam che possiedi ma non hai installato. Gratuita:
                steamcommunity.com/dev/apikey (come dominio puoi scrivere localhost).
              </span>
            </div>
            <div className="setting-inline">
              <input
                className="text-input mono"
                data-nav
                value={steamKey}
                placeholder="Incolla la chiave"
                spellCheck={false}
                onChange={(e) => setSteamKey(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && void saveSteamKey()}
              />
              <button type="button" className="btn" data-nav onClick={() => void saveSteamKey()}>
                Salva
              </button>
            </div>
          </div>
        </Section>

        <Section title="Copertine">
          <div className="setting-row is-static">
            <div className="setting-text">
              <span className="setting-label">Chiave API SteamGridDB</span>
              <span className="setting-hint">
                Serve per le copertine dei giochi non Steam. È gratuita: steamgriddb.com → Preferenze → API
              </span>
            </div>
            <div className="setting-inline">
              <input
                className="text-input mono"
                data-nav
                value={key}
                placeholder="Incolla la chiave"
                spellCheck={false}
                onChange={(e) => setKey(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && void saveKey()}
              />
              <button type="button" className="btn" data-nav onClick={() => void saveKey()}>
                Salva
              </button>
            </div>
          </div>
          <Row
            label="Scarica di nuovo le copertine mancanti"
            onClick={() => {
              void api.refreshArtwork();
              toast("Download delle copertine avviato");
            }}
          />
        </Section>

        <Section title="Giochi ed emulatori aggiunti a mano">
          {manual.map((m) => (
            <Row
              key={m.id}
              label={m.name}
              hint={[m.path, ...m.args].join(" ")}
              onClick={() => void removeManual(m)}
              control={<span className="setting-action">Rimuovi</span>}
            />
          ))}
          {draft ? (
            <div className="setting-row is-static draft">
              <span className="setting-hint mono">{draft.path}</span>
              <label className="field">
                <span>Nome</span>
                <input
                  className="text-input"
                  data-nav
                  data-nav-id="draft-name"
                  value={draft.name}
                  onChange={(e) => setDraft({ ...draft, name: e.target.value })}
                />
              </label>
              <label className="field">
                <span>Argomenti (facoltativi, es. ROM per un emulatore)</span>
                <input
                  className="text-input mono"
                  data-nav
                  value={draft.args}
                  placeholder='-L core.dll "C:\ROM\gioco.sfc"'
                  onChange={(e) => setDraft({ ...draft, args: e.target.value })}
                />
              </label>
              <div className="button-row">
                <button type="button" className="btn btn-primary" data-nav onClick={() => void saveDraft()}>
                  Aggiungi
                </button>
                <button type="button" className="btn" data-nav onClick={() => setDraft(null)}>
                  Annulla
                </button>
              </div>
            </div>
          ) : (
            <Row label="Aggiungi un gioco o un emulatore…" hint="Scegli un file .exe" onClick={() => void startAdd()} />
          )}
        </Section>

        <EmulatorSettings />

        <Section title="Salvataggi">
          <Row
            label="Cartella dei backup"
            hint={settings.backupDir ?? "Cartella dati di GameMode (consigliato un disco esterno o OneDrive)"}
            onClick={async () => {
              const dir = await pickFolder();
              if (dir) await saveSettings({ ...settings, backupDir: dir });
            }}
            control={<span className="setting-action">Cambia</span>}
          />
          <Row
            label={backingUp ? "Backup in corso…" : "Backup di tutti i giochi"}
            hint="Copia i salvataggi di ogni gioco installato che li tiene sul PC"
            onClick={async () => {
              if (backingUp) return;
              setBackingUp(true);
              try {
                const r = await api.backupAll();
                toast(`Backup fatto per ${r.saved} giochi${r.skipped ? ` (${r.skipped} senza salvataggi da copiare)` : ""}`);
              } catch (e) {
                toast(String(e), "error");
              } finally {
                setBackingUp(false);
              }
            }}
          />
          <p className="setting-hint settings-note">
            I percorsi dei salvataggi vengono dal Ludusavi Manifest, basato su PCGamingWiki.
          </p>
        </Section>

        <UpdateSettings />

        {hidden.length > 0 && (
          <Section title="Giochi nascosti">
            {hidden.map((g) => (
              <Row
                key={g.id}
                label={g.name}
                hint={STORE_LABEL[g.store]}
                onClick={() => {
                  void toggleHidden(g);
                  requestAnimationFrame(ensureFocus);
                }}
                control={<span className="setting-action">Mostra</span>}
              />
            ))}
          </Section>
        )}
      </div>
    </div>
  );
}

/** Splits a command line, keeping "quoted parts" together. */
function splitArgs(s: string): string[] {
  return [...s.matchAll(/"([^"]*)"|(\S+)/g)].map((m) => m[1] ?? m[2]);
}
