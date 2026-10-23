import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";

import { api } from "../lib/api";
import type { AudioOutput, DisplayInfo, DisplayMode, Settings } from "../lib/types";
import { useApp } from "../store/app";
import { ActionSheet, type SheetAction } from "./ActionSheet";
import { Row, Section } from "./SettingsRows";
import { Toggle } from "./common";

const label = (m: DisplayMode) => `${m.width} × ${m.height} · ${m.refresh} Hz`;

/** Idle minutes before the automatic sleep. */
export const SLEEP_CHOICES: { minutes: number | null; label: string }[] = [
  { minutes: null, label: "Mai" },
  { minutes: 15, label: "Dopo 15 minuti" },
  { minutes: 30, label: "Dopo 30 minuti" },
  { minutes: 60, label: "Dopo un'ora" },
];

export const sleepLabel = (minutes: number | null) => SLEEP_CHOICES.find((c) => c.minutes === minutes)?.label ?? `Dopo ${minutes} minuti`;

/** TV mode (display + audio output) and performance mode. */
export function SystemSettings({ settings }: { settings: Settings }) {
  const saveSettings = useApp((s) => s.saveSettings);
  const toast = useApp((s) => s.toast);
  const [display, setDisplay] = useState<DisplayInfo | null>(null);
  const [outputs, setOutputs] = useState<AudioOutput[]>([]);
  const [sheet, setSheet] = useState<"display" | "audio" | "sleep" | null>(null);
  const [apps, setApps] = useState(settings.closeApps.join(", "));

  useEffect(() => {
    void api.displayModes().then(setDisplay).catch(() => {});
    void api.audioOutputs().then(setOutputs).catch(() => {});
  }, []);

  const save = async (patch: Partial<Settings>) => {
    await saveSettings({ ...settings, ...patch });
    void api.applyTvMode();
  };

  const saveApps = async () => {
    const closeApps = apps
      .split(/[,;\n]/)
      .map((a) => a.trim())
      .filter(Boolean)
      .map((a) => (a.toLowerCase().endsWith(".exe") ? a : `${a}.exe`));
    await saveSettings({ ...settings, closeApps });
    setApps(closeApps.join(", "));
    toast(closeApps.length ? `Durante il gioco chiuderò: ${closeApps.join(", ")}` : "Nessuna app da chiudere");
  };

  const displayActions: SheetAction[] = [
    { label: "Come il desktop", hint: display?.current ? label(display.current) : undefined, onSelect: () => void save({ tvMode: null }) },
    ...(display?.modes ?? []).map((m) => ({ label: label(m), onSelect: () => void save({ tvMode: m }) })),
  ];
  const audioActions: SheetAction[] = [
    { label: "Non cambiare uscita", onSelect: () => void save({ tvAudio: null }) },
    ...outputs.map((o) => ({ label: o.name, hint: o.default ? "In uso adesso" : undefined, onSelect: () => void save({ tvAudio: o.id }) })),
  ];
  const audioName = outputs.find((o) => o.id === settings.tvAudio)?.name;
  const sleepActions: SheetAction[] = SLEEP_CHOICES.map(({ minutes, label }) => ({
    label,
    hint: minutes === settings.autoSleepMinutes ? "Scelta attuale" : undefined,
    onSelect: () => void saveSettings({ ...settings, autoSleepMinutes: minutes }),
  }));

  return (
    <>
      <Section title="Modalità TV">
        <Row
          label="Risoluzione di GameMode"
          hint={settings.tvMode ? label(settings.tvMode) : "Come il desktop"}
          onClick={() => setSheet("display")}
          control={<span className="setting-action">Cambia</span>}
        />
        <Row
          label="Uscita audio di GameMode"
          hint={settings.tvAudio ? (audioName ?? "Dispositivo non collegato") : "Non cambiare uscita"}
          onClick={() => setSheet("audio")}
          control={<span className="setting-action">Cambia</span>}
        />
        <p className="setting-hint settings-note">
          Si applicano a schermo intero e si annullano quando vai al desktop. I giochi scelgono comunque la loro risoluzione.
        </p>
      </Section>

      <Section title="Prestazioni">
        <Row
          label="Modalità prestazioni durante il gioco"
          hint="Piano energetico Prestazioni elevate e chiusura delle app qui sotto; tutto torna com'era a fine partita"
          onClick={() => void saveSettings({ ...settings, performanceMode: !settings.performanceMode })}
          control={<Toggle on={settings.performanceMode} />}
        />
        <div className="setting-row is-static">
          <div className="setting-text">
            <span className="setting-label">App da chiudere</span>
            <span className="setting-hint">Separate da virgole, es. chrome, msedge, discord. Vengono chiuse come premendo la X.</span>
          </div>
          <div className="setting-inline">
            <input
              className="text-input mono"
              data-nav
              aria-label="App da chiudere durante il gioco"
              placeholder="chrome.exe, msedge.exe"
              value={apps}
              spellCheck={false}
              onChange={(e) => setApps(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void saveApps()}
            />
            <button type="button" className="btn" data-nav onClick={() => void saveApps()}>
              Salva
            </button>
          </div>
        </div>
      </Section>

      <Section title="Risparmio energetico">
        <Row
          label="Sospendi il PC se inattivo"
          hint={sleepLabel(settings.autoSleepMinutes)}
          onClick={() => setSheet("sleep")}
          control={<span className="setting-action">Cambia</span>}
        />
        <p className="setting-hint settings-note">
          Conta anche il controller, che Windows ignora. Un minuto prima compare un avviso; durante una partita il PC resta
          acceso.
        </p>
      </Section>

      <AnimatePresence>
        {sheet === "display" && <ActionSheet key="display" title="Risoluzione" actions={displayActions} onClose={() => setSheet(null)} />}
        {sheet === "audio" && <ActionSheet key="audio" title="Uscita audio" actions={audioActions} onClose={() => setSheet(null)} />}
        {sheet === "sleep" && <ActionSheet key="sleep" title="Sospensione automatica" actions={sleepActions} onClose={() => setSheet(null)} />}
      </AnimatePresence>
    </>
  );
}
