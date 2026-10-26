import { api } from "../lib/api";
import { useApp } from "../store/app";
import { Row, Section } from "./SettingsRows";
import { Toggle } from "./common";

/** Screenshots: the controller combo and where they go. */
export function CaptureSettings() {
  const settings = useApp((s) => s.settings);
  const saveSettings = useApp((s) => s.saveSettings);
  const toast = useApp((s) => s.toast);

  if (!settings) return null;

  return (
    <Section title="Catture">
      <Row
        label="View+RB per una schermata"
        hint="Durante un gioco, tieni premuti View e RB per salvare una schermata. La trovi nella pagina del gioco e nella Home"
        onClick={() => void saveSettings({ ...settings, captureCombo: !settings.captureCombo })}
        control={<Toggle on={settings.captureCombo} />}
      />
      <Row
        label="Apri la cartella delle catture"
        hint="Immagini → GameMode, una cartella per gioco"
        onClick={() => void api.openCapturesFolder().catch((e) => toast(String(e), "error"))}
        control={<span className="setting-action">Apri</span>}
      />
      <p className="setting-hint settings-note">
        Le schermate si scattano anche dall'overlay (tasto Xbox → Scatta una schermata). Per le clip video usa Xbox
        Game Bar, se è installata: Win+Alt+R avvia e ferma la registrazione.
      </p>
    </Section>
  );
}
