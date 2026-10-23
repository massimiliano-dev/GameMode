import { useEffect, useState } from "react";

import { api } from "../lib/api";
import { formatClock, formatDate } from "../lib/format";
import type { StartupInfo } from "../lib/types";
import { useApp } from "../store/app";
import { Row, Section } from "./SettingsRows";

const LOG_LINES = 120;

/** How GameMode came up, the last crash, and the log (view, export, forget). */
export function DiagnosticsSettings() {
  const toast = useApp((s) => s.toast);
  const [info, setInfo] = useState<StartupInfo | null>(null);
  const [log, setLog] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);

  const refresh = () => void api.startupInfo().then(setInfo).catch(() => {});
  useEffect(refresh, []);

  const startHint = !info
    ? ""
    : info.safeMode
      ? "Modalità provvisoria: il watchdog ha aperto il desktop dopo troppi errori di seguito"
      : info.recovered > 0
        ? `Riavviato dal watchdog dopo ${info.recovered === 1 ? "un errore" : `${info.recovered} errori di seguito`}`
        : "Avvio normale";
  const crash = info?.lastCrash ?? null;
  const when = crash ? new Date(crash.at * 1000) : null;
  const crashHint = crash && when
    ? `${info?.lastCrashDescription ?? "Errore"} · ${formatDate(when)} alle ${formatClock(when)}${
        info && info.crashCount > 1 ? ` · ${info.crashCount} errori registrati` : ""
      }`
    : "Nessun errore registrato";

  const toggleLog = async () => {
    if (log !== null) return setLog(null);
    setLog(await api.readLog(LOG_LINES).catch(() => ""));
  };

  const exportLog = async () => {
    if (exporting) return;
    setExporting(true);
    try {
      const path = await api.exportLog();
      if (path) toast(`Log salvato in ${path}`);
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setExporting(false);
    }
  };

  const forget = async () => {
    await api.clearCrashes().catch(() => {});
    toast("Errori registrati dimenticati");
    refresh();
  };

  return (
    <Section title="Diagnostica">
      <div className="setting-row is-static">
        <div className="setting-text">
          <span className="setting-label">Questo avvio</span>
          <span className="setting-hint">{startHint}</span>
        </div>
      </div>
      <div className="setting-row is-static">
        <div className="setting-text">
          <span className="setting-label">Ultimo errore</span>
          <span className="setting-hint">{crashHint}</span>
          {info?.lastPanic && <pre className="diag-log">{info.lastPanic}</pre>}
        </div>
      </div>
      <Row
        label={log === null ? "Mostra il log" : "Nascondi il log"}
        hint={info?.logPath ?? "Le ultime righe di quello che GameMode e il watchdog hanno annotato"}
        onClick={() => void toggleLog()}
      />
      {log !== null && <pre className="diag-log">{log || "Il log è vuoto"}</pre>}
      <Row
        label={exporting ? "Esportazione…" : "Esporta il log"}
        hint="Salva una copia da allegare a una segnalazione"
        onClick={() => void exportLog()}
      />
      {!!info?.crashCount && (
        <Row label="Dimentica gli errori registrati" hint="Azzera il contatore del watchdog" onClick={() => void forget()} />
      )}
    </Section>
  );
}
