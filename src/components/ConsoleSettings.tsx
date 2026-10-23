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
  const [restart, setRestart] = useState<"desktop" | "console" | null>(null);

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

  const reboot = async (action: "restart" | "restartToDesktop") => {
    try {
      await api.power(action);
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
            : status.desktopOnce
              ? "Attiva: questa sessione è sul desktop, al prossimo riavvio torni alla console"
              : status.enabled
                ? "Si attiva dal prossimo accesso a Windows"
                : "Accedendo a Windows trovi subito GameMode, senza desktop né barra delle applicazioni"
        }
        onClick={() => (status.enabled ? void apply(false) : setConfirm(true))}
        control={<Toggle on={status.enabled} />}
      />
      {status.active && (
        <Row
          label="Riavvia in modalità desktop"
          hint="Una volta sola: al riavvio successivo torni in GameMode. In alternativa tieni premuti B + Visualizza mentre GameMode si avvia"
          onClick={() => setRestart("desktop")}
        />
      )}
      {status.desktopOnce && (
        <Row label="Torna alla console" hint="Riavvia il PC: all'accesso trovi di nuovo GameMode" onClick={() => setRestart("console")} />
      )}
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
            text="Dal prossimo accesso a Windows vedrai solo GameMode: niente desktop, barra o menu Start. I programmi che partono con Windows li avvia GameMode. Per il desktop usa Menu rapido → Vai al desktop; se GameMode si chiude per errore il desktop riparte da solo. Puoi disattivarla qui in ogni momento."
            confirmLabel="Attiva"
            onCancel={() => setConfirm(false)}
            onConfirm={() => {
              setConfirm(false);
              void apply(true);
            }}
          />
        )}
        {restart && (
          <ConfirmDialog
            key="restart"
            title={restart === "desktop" ? "Riavviare in modalità desktop?" : "Riavviare in modalità console?"}
            text={
              restart === "desktop"
                ? "Il PC si riavvia e questa volta Windows apre il desktop normale, con GameMode nell'area di notifica. La modalità console resta attiva: al riavvio successivo torni in GameMode."
                : "Il PC si riavvia e all'accesso trovi di nuovo GameMode al posto del desktop."
            }
            confirmLabel="Riavvia"
            onCancel={() => setRestart(null)}
            onConfirm={() => {
              setRestart(null);
              void reboot(restart === "desktop" ? "restartToDesktop" : "restart");
            }}
          />
        )}
      </AnimatePresence>
    </Section>
  );
}
