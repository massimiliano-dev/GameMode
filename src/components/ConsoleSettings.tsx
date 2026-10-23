import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";

import { api } from "../lib/api";
import type { ShellStatus } from "../lib/types";
import { useApp } from "../store/app";
import { ConfirmDialog } from "./ConfirmDialog";
import { Row, Section } from "./SettingsRows";
import { Toggle } from "./common";

/** Console mode: GameMode instead of the Windows desktop, for this user. */
export function ConsoleSettings() {
  const toast = useApp((s) => s.toast);
  const [status, setStatus] = useState<ShellStatus | null>(null);
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    void api.getConsoleMode().then(setStatus).catch(() => {});
  }, []);

  const apply = async (enabled: boolean) => {
    try {
      setStatus(await api.setConsoleMode(enabled));
      toast(
        enabled
          ? "Modalità console attiva dal prossimo accesso a Windows"
          : "Dal prossimo accesso Windows torna al desktop normale",
      );
    } catch (e) {
      toast(String(e), "error");
    }
  };

  if (!status) return null;

  return (
    <Section title="Modalità console">
      <Row
        label="Avvia GameMode al posto del desktop"
        hint={
          status.active
            ? "Attiva: questa sessione è partita direttamente in GameMode"
            : status.enabled
              ? "Si attiva dal prossimo accesso a Windows"
              : "Accedendo a Windows trovi subito GameMode, senza desktop né barra delle applicazioni"
        }
        onClick={() => (status.enabled ? void apply(false) : setConfirm(true))}
        control={<Toggle on={status.enabled} />}
      />
      <p className="setting-hint settings-note">
        Vale solo per questo utente di Windows. Per avere un PC "console" separato dal tuo desktop di lavoro, crea un
        utente Windows dedicato (Impostazioni di Windows → Account → Altri utenti), accedi con quello, apri GameMode e
        attiva qui la modalità console. Per l'accesso automatico a quell'utente usa netplwiz.
      </p>
      <AnimatePresence>
        {confirm && (
          <ConfirmDialog
            key="console"
            title="Attivare la modalità console?"
            text="Dal prossimo accesso a Windows vedrai solo GameMode: niente desktop, barra o menu Start. I programmi che partono con Windows li avvia GameMode. Per il desktop usa Menu rapido → Vai al desktop; se GameMode si chiude per errore viene riavviato da solo e, dopo più errori di seguito, si apre il desktop in modalità provvisoria. Puoi disattivarla qui in ogni momento."
            confirmLabel="Attiva"
            onCancel={() => setConfirm(false)}
            onConfirm={() => {
              setConfirm(false);
              void apply(true);
            }}
          />
        )}
      </AnimatePresence>
    </Section>
  );
}
