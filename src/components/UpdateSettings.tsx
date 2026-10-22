import { AnimatePresence } from "motion/react";
import { useEffect, useState } from "react";

import { api } from "../lib/api";
import { useApp } from "../store/app";
import { ConfirmDialog } from "./ConfirmDialog";
import { Row, Section } from "./SettingsRows";

export function UpdateSettings() {
  const update = useApp((s) => s.update);
  const toast = useApp((s) => s.toast);
  const [version, setVersion] = useState("");
  const [state, setState] = useState<"idle" | "checking" | "installing">("idle");
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    void api.appVersion().then(setVersion);
  }, []);

  const check = async () => {
    setState("checking");
    try {
      const found = await api.checkUpdate();
      useApp.setState({ update: found });
      if (!found) toast("Hai già l'ultima versione");
    } catch (e) {
      toast(`Controllo non riuscito: ${String(e)}`, "error");
    } finally {
      setState("idle");
    }
  };

  const install = async () => {
    setState("installing");
    try {
      // On success GameMode restarts and this never returns.
      await api.installUpdate();
    } catch (e) {
      toast(String(e), "error");
      setState("idle");
    }
  };

  return (
    <Section title="Aggiornamenti">
      {update ? (
        <Row
          label={state === "installing" ? "Scarico e installo…" : `Installa GameMode ${update.version}`}
          hint={update.notes ?? `Versione attuale ${version}`}
          onClick={() => state === "idle" && setConfirm(true)}
          control={<span className="setting-action">Nuova versione</span>}
        />
      ) : (
        <Row
          label={state === "checking" ? "Controllo in corso…" : "Cerca aggiornamenti"}
          hint={`Versione attuale ${version}. GameMode controlla da solo a ogni avvio.`}
          onClick={() => state === "idle" && void check()}
        />
      )}
      <AnimatePresence>
        {confirm && update && (
          <ConfirmDialog
            key="update"
            title={`Installare GameMode ${update.version}?`}
            text="GameMode si chiude, si aggiorna e si riapre da solo. Libreria e impostazioni restano."
            confirmLabel="Aggiorna"
            onCancel={() => setConfirm(false)}
            onConfirm={() => {
              setConfirm(false);
              void install();
            }}
          />
        )}
      </AnimatePresence>
    </Section>
  );
}
