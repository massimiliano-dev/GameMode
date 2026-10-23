import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { ensureFocus, focusInitial } from "../input/focus";
import { setupInput } from "../input/sources";
import { api } from "../lib/api";
import { formatClock, formatDate } from "../lib/format";
import type { StartupInfo } from "../lib/types";

const RELEASES = "https://github.com/massimiliano-dev/GameMode/releases/latest";
const LOG_LINES = 40;

/**
 * Shown instead of the launcher when the watchdog gave up restarting
 * GameMode (crashes in a row): Explorer is already running, so the user
 * can get out of console mode, read what happened or carry on anyway.
 */
export function SafeMode({ info, onContinue }: { info: StartupInfo; onContinue: () => void }) {
  const [log, setLog] = useState<string | null>(null);
  const [consoleMode, setConsoleMode] = useState<boolean | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const logRef = useRef<HTMLPreElement>(null);

  useEffect(() => {
    void setupInput();
    void api.readLog(LOG_LINES).then(setLog).catch(() => setLog(""));
    void api
      .getConsoleMode()
      .then((s) => setConsoleMode(s.enabled))
      .catch(() => setConsoleMode(null));
    requestAnimationFrame(() => focusInitial());
  }, []);
  // The newest lines matter most: start at the bottom (the controller can't scroll it).
  useLayoutEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [log]);

  const crash = info.lastCrash;
  const when = crash ? new Date(crash.at * 1000) : null;

  const disableConsole = async () => {
    try {
      await api.setConsoleMode(false);
      setConsoleMode(false);
      // The button just pressed goes away: give the focus back.
      requestAnimationFrame(ensureFocus);
      setNote("Modalità console disattivata: al prossimo accesso Windows parte con il desktop");
    } catch (e) {
      setNote(String(e));
    }
  };

  const exportLog = async () => {
    try {
      const path = await api.exportLog();
      setNote(path ? `Log salvato in ${path}` : null);
    } catch (e) {
      setNote(String(e));
    }
  };

  return (
    <div className="screen safe-mode">
      <span className="safe-mode-badge">Modalità provvisoria</span>
      <h1 className="screen-title">GameMode si è chiuso più volte di seguito</h1>
      <p>
        Il watchdog ha riavviato GameMode {info.recovered > 0 ? `${info.recovered} volte` : "più volte"} senza successo, così
        questa volta ha aperto prima il desktop di Windows. La modalità console resta sospesa per questa sessione; i giochi
        e la libreria sono al sicuro.
      </p>
      {crash && (
        <div className="safe-mode-crash">
          <strong>{info.lastCrashDescription ?? "Errore"}</strong>
          {when && (
            <span className="setting-hint">
              {formatDate(when)} alle {formatClock(when)} · codice 0x{crash.exitCode.toString(16).toUpperCase()}
            </span>
          )}
          {info.lastPanic && <pre className="diag-log">{info.lastPanic}</pre>}
        </div>
      )}
      <div className="button-row">
        <button type="button" className="btn btn-primary" data-nav data-nav-autofocus onClick={onContinue}>
          Continua comunque
        </button>
        {consoleMode && (
          <button type="button" className="btn" data-nav onClick={() => void disableConsole()}>
            Disattiva modalità console
          </button>
        )}
        <button type="button" className="btn" data-nav onClick={() => void api.exitToDesktop()}>
          Vai al desktop
        </button>
        <button type="button" className="btn" data-nav onClick={() => void api.openLink(RELEASES)}>
          Scarica l'ultima versione
        </button>
        <button type="button" className="btn" data-nav onClick={() => void exportLog()}>
          Esporta il log
        </button>
        <button type="button" className="btn btn-danger" data-nav onClick={() => void api.quit()}>
          Chiudi GameMode
        </button>
      </div>
      {note && <p className="setting-hint">{note}</p>}
      <section>
        <h2 className="row-title">Ultime righe del log</h2>
        <pre ref={logRef} className="diag-log">
          {log || "Il log è vuoto"}
        </pre>
        <p className="setting-hint mono">{info.logPath}</p>
      </section>
    </div>
  );
}
