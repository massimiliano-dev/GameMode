import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";

import { api } from "../lib/api";
import type { AudioOutput, DisplayInfo, DisplayMode, PowerAction, Settings } from "../lib/types";
import { useApp } from "../store/app";
import { ActionSheet, type SheetAction } from "./ActionSheet";
import { Row, Section } from "./SettingsRows";
import { Toggle } from "./common";

const label = (m: DisplayMode) => `${m.width} × ${m.height} · ${m.refresh} Hz`;

const IDLE_MINUTES = [0, 5, 10, 15, 30, 60, 120];
const minutes = (m: number) => (m === 0 ? "Mai" : m < 60 ? `${m} minuti` : m === 60 ? "1 ora" : `${m / 60} ore`);
const IDLE_ACTION: Record<PowerAction, string> = { sleep: "Sospendi il PC", shutdown: "Spegni il PC", restart: "Riavvia il PC", restartToDesktop: "Riavvia in modalità desktop" };
type IdleKey = "idleScreensaverMin" | "idleDisplayOffMin" | "idleSleepMin";

/** TV mode (display + audio output) and performance mode. */
export function SystemSettings({ settings }: { settings: Settings }) {
  const saveSettings = useApp((s) => s.saveSettings);
  const toast = useApp((s) => s.toast);
  const [display, setDisplay] = useState<DisplayInfo | null>(null);
  const [outputs, setOutputs] = useState<AudioOutput[]>([]);
  const [sheet, setSheet] = useState<"display" | "audio" | IdleKey | null>(null);
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
  const idleActions = (key: IdleKey): SheetAction[] =>
    IDLE_MINUTES.map((m) => ({ label: minutes(m), hint: settings[key] === m ? "Adesso" : undefined, onSelect: () => void saveSettings({ ...settings, [key]: m }) }));

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

      <Section title="Inattività e alimentazione">
        <Row
          label="Salvaschermo dopo"
          hint="Orologio e copertine dei giochi più giocati; qualsiasi tasto lo chiude"
          onClick={() => setSheet("idleScreensaverMin")}
          control={<span className="setting-action">{minutes(settings.idleScreensaverMin)}</span>}
        />
        <Row
          label="Spegni lo schermo dopo"
          hint="Il PC resta acceso; il controller, il mouse o la tastiera lo riaccendono"
          onClick={() => setSheet("idleDisplayOffMin")}
          control={<span className="setting-action">{minutes(settings.idleDisplayOffMin)}</span>}
        />
        <Row
          label={`${settings.idleAction === "shutdown" ? "Spegni" : "Sospendi"} il PC dopo`}
          hint="Mai durante una partita, con un gioco in pausa escluso, o mentre un'altra app è in primo piano"
          onClick={() => setSheet("idleSleepMin")}
          control={<span className="setting-action">{minutes(settings.idleSleepMin)}</span>}
        />
        <Row
          label="Cosa fare dopo l'inattività"
          hint="La sospensione riprende in pochi secondi; lo spegnimento chiude tutto"
          onClick={() => void saveSettings({ ...settings, idleAction: settings.idleAction === "shutdown" ? "sleep" : "shutdown" })}
          control={<span className="setting-action">{IDLE_ACTION[settings.idleAction]}</span>}
        />
        <Row
          label="Aspetta i download"
          hint="Finché un gioco scarica o si aggiorna il PC resta sveglio (lo schermo può spegnersi)"
          onClick={() => void saveSettings({ ...settings, idleWaitDownloads: !settings.idleWaitDownloads })}
          control={<Toggle on={settings.idleWaitDownloads} />}
        />
        <p className="setting-hint settings-note">
          Il controller conta come attività. Se Windows chiede la password al risveglio serve la tastiera: puoi disattivarla in Impostazioni di Windows → Account → Opzioni di accesso. Anche il piano energetico di Windows ha i suoi tempi: vince il più breve.
        </p>
      </Section>

      <AnimatePresence>
        {sheet === "display" && <ActionSheet key="display" title="Risoluzione" actions={displayActions} onClose={() => setSheet(null)} />}
        {sheet === "audio" && <ActionSheet key="audio" title="Uscita audio" actions={audioActions} onClose={() => setSheet(null)} />}
        {sheet === "idleScreensaverMin" && <ActionSheet key="saver" title="Salvaschermo dopo" actions={idleActions("idleScreensaverMin")} onClose={() => setSheet(null)} />}
        {sheet === "idleDisplayOffMin" && <ActionSheet key="off" title="Spegni lo schermo dopo" actions={idleActions("idleDisplayOffMin")} onClose={() => setSheet(null)} />}
        {sheet === "idleSleepMin" && (
          <ActionSheet key="sleep" title={`${settings.idleAction === "shutdown" ? "Spegni" : "Sospendi"} il PC dopo`} actions={idleActions("idleSleepMin")} onClose={() => setSheet(null)} />
        )}
      </AnimatePresence>
    </>
  );
}
