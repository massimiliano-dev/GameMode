import { useState } from "react";

import { api } from "../lib/api";
import { useApp } from "../store/app";
import { Row, Section } from "./SettingsRows";

/** IGDB credentials and a manual refresh of the game cards. */
export function MetadataSettings() {
  const settings = useApp((s) => s.settings);
  const saveSettings = useApp((s) => s.saveSettings);
  const toast = useApp((s) => s.toast);
  const [clientId, setClientId] = useState(settings?.igdbClientId ?? "");
  const [secret, setSecret] = useState(settings?.igdbClientSecret ?? "");

  if (!settings) return null;

  const save = async () => {
    const id = clientId.trim() || null;
    const key = secret.trim() || null;
    await saveSettings({ ...settings, igdbClientId: id, igdbClientSecret: key });
    toast(id && key ? "Chiavi IGDB salvate, completo le schede dei giochi" : "Chiavi IGDB rimosse");
  };

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
            onKeyDown={(e) => e.key === "Enter" && void save()}
          />
          <input
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
          toast("Ricerca delle schede avviata");
        }}
      />
    </Section>
  );
}
