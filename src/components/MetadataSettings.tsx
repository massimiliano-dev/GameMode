import { useRef, useState } from "react";

import { focusElement } from "../input/focus";
import { api } from "../lib/api";
import { useMetadataRunning } from "../lib/metaStatus";
import { useApp } from "../store/app";
import { Row, Section } from "./SettingsRows";

/** IGDB credentials and a manual refresh of the game cards. */
export function MetadataSettings() {
  const settings = useApp((s) => s.settings);
  const saveSettings = useApp((s) => s.saveSettings);
  const toast = useApp((s) => s.toast);
  const [clientId, setClientId] = useState(settings?.igdbClientId ?? "");
  const [secret, setSecret] = useState(settings?.igdbClientSecret ?? "");
  const secretRef = useRef<HTMLInputElement>(null);
  const searching = useMetadataRunning();

  if (!settings) return null;

  const save = async () => {
    const id = clientId.trim() || null;
    const key = secret.trim() || null;
    // Half a pair is useless to Twitch: wait for the other field.
    if (!!id !== !!key) {
      toast(id ? "Inserisci anche il Client Secret" : "Inserisci anche il Client ID");
      return;
    }
    const had = !!(settings.igdbClientId && settings.igdbClientSecret);
    if (id === (settings.igdbClientId ?? null) && key === (settings.igdbClientSecret ?? null)) {
      if (id) toast("Chiavi IGDB già salvate");
      return;
    }
    await saveSettings({ ...settings, igdbClientId: id, igdbClientSecret: key });
    if (id && key) toast("Chiavi IGDB salvate, completo le schede dei giochi");
    else if (had) toast("Chiavi IGDB rimosse");
  };

  // Enter on the ID (also "Fatto" on the on-screen keyboard) moves on to the
  // secret. The keyboard hands focus back to the field in the next frame, so
  // the move waits for that.
  const nextField = () =>
    requestAnimationFrame(() => {
      if (secretRef.current) focusElement(secretRef.current);
    });

  return (
    <Section title="Scheda gioco">
      <div className="setting-row is-static">
        <div className="setting-text">
          <span className="setting-label">Chiavi IGDB (facoltative)</span>
          <span className="setting-hint">
            Descrizione, generi e voto arrivano da Steam, Microsoft Store e GOG senza chiavi. Con un'app gratuita su
            dev.twitch.tv (Client ID e Client Secret) la scheda si completa con il tempo per finire il gioco e copre
            anche Epic, EA, Ubisoft ed emulatori.
          </span>
        </div>
        <div className="setting-inline">
          <input
            className="text-input mono"
            data-nav
            value={clientId}
            placeholder="Client ID"
            spellCheck={false}
            onChange={(e) => setClientId(e.target.value)}
            onKeyDown={(e) => {
              if (e.key !== "Enter") return;
              if (secret.trim() || !clientId.trim()) void save();
              else nextField();
            }}
          />
          <input
            ref={secretRef}
            className="text-input mono"
            data-nav
            value={secret}
            placeholder="Client Secret"
            spellCheck={false}
            onChange={(e) => setSecret(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void save()}
          />
          <button type="button" className="btn" data-nav onClick={() => void save()}>
            Salva
          </button>
        </div>
      </div>
      <Row
        label="Cerca di nuovo le schede mancanti"
        hint="Riprova per i giochi senza descrizione e completa quelle parziali"
        onClick={() => {
          void api.refreshMetadata();
          toast(searching ? "Ricerca già in corso: riprovo appena finisce" : "Ricerca delle schede avviata");
        }}
      />
    </Section>
  );
}
