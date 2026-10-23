import { useEffect, useState } from "react";

import { api } from "../lib/api";
import { useApp } from "../store/app";
import { Row, Section } from "./SettingsRows";
import { Toggle } from "./common";

/** Wand (formerly WeMod): its overlay from GameMode's, driven by the controller. */
export function WandSettings() {
  const settings = useApp((s) => s.settings);
  const saveSettings = useApp((s) => s.saveSettings);
  const [installed, setInstalled] = useState(false);

  useEffect(() => {
    void api.wandInstalled().then(setInstalled).catch(() => {});
  }, []);

  if (!settings || !installed) return null;

  return (
    <Section title="Wand (mod)">
      <Row
        label="Apri Wand"
        hint="Scegli il gioco e premi Play: Wand lo avvia con le mod, o si aggancia se è già aperto. Col controller: croce e A, B indietro, View torna a GameMode"
        onClick={() => void api.openWandApp()}
      />
      <Row
        label="Scorciatoia dell'overlay di Wand"
        hint="Deve essere la stessa scelta in Wand → Impostazioni → Personalizzazione"
        onClick={() => void saveSettings({ ...settings, wandBacktick: !settings.wandBacktick })}
        control={<span className="setting-action">{settings.wandBacktick ? "Alt + `" : "Alt + W"}</span>}
      />
      <Row
        label="LB+RB per aprire Wand"
        hint="Durante un gioco, tieni premuti LB e RB per aprire o chiudere l'overlay di Wand. Lo apri anche dall'overlay di GameMode → Overlay di Wand"
        onClick={() => void saveSettings({ ...settings, wandCombo: !settings.wandCombo })}
        control={<Toggle on={settings.wandCombo} />}
      />
    </Section>
  );
}
