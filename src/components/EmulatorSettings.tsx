import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";

import { api, pickExecutable, pickFolder } from "../lib/api";
import type { EmulatorStatus, SystemStatus } from "../lib/types";
import { useApp } from "../store/app";
import { ActionSheet } from "./ActionSheet";
import { Row, Section } from "./SettingsRows";

/** RetroArch location and one ROM folder per emulated system. */
export function EmulatorSettings() {
  const rescan = useApp((s) => s.rescan);
  const toast = useApp((s) => s.toast);
  const [status, setStatus] = useState<EmulatorStatus | null>(null);
  const [editing, setEditing] = useState<SystemStatus | null>(null);

  useEffect(() => {
    void api.emulatorStatus().then(setStatus).catch(() => {});
  }, []);

  const apply = async (call: Promise<EmulatorStatus>) => {
    try {
      setStatus(await call);
      void rescan();
    } catch (e) {
      toast(String(e), "error");
    }
  };

  const chooseRetroarch = async () => {
    const path = await pickExecutable();
    if (path) await apply(api.setRetroarchPath(path));
  };

  const chooseFolder = async (system: SystemStatus) => {
    const path = await pickFolder();
    if (path) await apply(api.setRomDir(system.key, path));
  };

  if (!status) return null;

  return (
    <Section title="Emulatori (RetroArch)">
      <Row
        label="RetroArch"
        hint={
          status.retroarch
            ? `${status.retroarch}${status.detected ? " · trovato in automatico" : ""}`
            : "Non trovato: installalo da retroarch.com oppure indica dove si trova"
        }
        onClick={() => void chooseRetroarch()}
        control={<span className="setting-action">{status.retroarch ? "Cambia" : "Scegli"}</span>}
      />
      {status.systems.map((s) => (
        <Row
          key={s.key}
          label={s.name}
          hint={systemHint(s, !!status.retroarch)}
          onClick={() => (s.path ? setEditing(s) : void chooseFolder(s))}
          control={<span className="setting-action">{s.path ? `${s.roms} ROM` : "Aggiungi cartella"}</span>}
        />
      ))}
      <AnimatePresence>
        {editing && (
          <ActionSheet
            key="system"
            title={editing.name}
            onClose={() => setEditing(null)}
            actions={[
              { label: "Cambia cartella", hint: editing.path ?? undefined, onSelect: () => void chooseFolder(editing) },
              { label: "Rimuovi cartella", hint: "Le ROM restano sul disco", danger: true, onSelect: () => void apply(api.setRomDir(editing.key, null)) },
            ]}
          />
        )}
      </AnimatePresence>
    </Section>
  );
}

function systemHint(s: SystemStatus, hasRetroarch: boolean) {
  const folder = s.path ?? "Nessuna cartella delle ROM";
  if (!hasRetroarch) return folder;
  if (!s.coreInstalled) return `${folder} · manca il core ${s.core}: in RetroArch apri Aggiornamenti online → Scarica core`;
  return folder;
}
