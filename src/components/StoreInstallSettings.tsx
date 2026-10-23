import { useEffect, useState } from "react";

import { api, pickFolder } from "../lib/api";
import type { GogStatus, Settings } from "../lib/types";
import { useApp } from "../store/app";
import { Row, Section } from "./SettingsRows";
import { Toggle } from "./common";

/** Installing without the store clients: GOG account and folder, Microsoft Store. */
export function StoreInstallSettings({ settings }: { settings: Settings }) {
  const saveSettings = useApp((s) => s.saveSettings);
  const toast = useApp((s) => s.toast);
  const [gog, setGog] = useState<GogStatus | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = () => void api.gogStatus().then(setGog).catch(() => {});
  useEffect(refresh, [settings.gogInstallDir]);

  const toggleAccount = async () => {
    if (busy || !gog) return;
    setBusy(true);
    try {
      if (gog.signedIn) {
        await api.gogLogout();
        toast("Account GOG scollegato");
      } else if (await api.gogLogin()) {
        toast("Account GOG collegato: i tuoi giochi compaiono in Scarica");
      }
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setBusy(false);
      refresh();
    }
  };

  const pickGogFolder = async () => {
    const dir = await pickFolder();
    if (dir) await saveSettings({ ...settings, gogInstallDir: dir });
  };

  return (
    <Section title="Installazioni senza launcher">
      <Row
        label={gog?.signedIn ? "Account GOG collegato" : "Collega account GOG"}
        hint={
          busy
            ? "Accedi nella finestra di GOG…"
            : gog?.signedIn
              ? "GameMode scarica e installa i giochi GOG da solo, senza GOG Galaxy"
              : "Per installare i giochi GOG senza GOG Galaxy"
        }
        onClick={() => void toggleAccount()}
        control={<span className="setting-action">{gog?.signedIn ? "Scollega" : "Accedi"}</span>}
      />
      <Row
        label="Cartella dei giochi GOG"
        hint={gog?.installDir ?? settings.gogInstallDir ?? ""}
        onClick={() => void pickGogFolder()}
        control={<span className="setting-action">Cambia</span>}
      />
      <Row
        label="Microsoft Store senza app Xbox"
        hint="Installa e aggiorna i giochi Game Pass e Store direttamente. Se non riesce si apre l'app Xbox"
        onClick={() => void saveSettings({ ...settings, msstoreDirect: !settings.msstoreDirect })}
        control={<Toggle on={settings.msstoreDirect} />}
      />
    </Section>
  );
}
