import { useEffect, useState } from "react";

import { api, on } from "../lib/api";
import type { LegendaryStatus, Settings } from "../lib/types";
import { useApp } from "../store/app";
import { Row, Section } from "./SettingsRows";
import { Toggle } from "./common";

type LoginResult = { Ok: string } | { Err: string };

/** Epic games without the Epic Games Launcher, through legendary. */
export function EpicSettings({ settings }: { settings: Settings }) {
  const toast = useApp((s) => s.toast);
  const [status, setStatus] = useState<LegendaryStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [download, setDownload] = useState<number | null>(null);

  const refresh = () => void api.legendaryStatus().then(setStatus).catch(() => {});
  useEffect(() => {
    refresh();
    const offs = [
      on("legendary://changed", refresh),
      on<number>("legendary://download", setDownload),
      on<LoginResult>("legendary://login", (r) => {
        if ("Ok" in r) toast(`Accesso a Epic come ${r.Ok}`);
        else toast(r.Err, "error");
        refresh();
      }),
    ];
    return () => offs.forEach((off) => off());
  }, []);

  if (!status?.supported) return null;
  const enabled = settings.epicLegendary && status.downloaded;

  /** Runs one action at a time, with a hint while it works. */
  const work = async (hint: string, f: () => Promise<void>) => {
    if (busy) return;
    setBusy(hint);
    try {
      await f();
    } catch (e) {
      toast(String(e), "error");
    } finally {
      setBusy(null);
      setDownload(null);
      refresh();
    }
  };

  const toggle = () =>
    work(status.downloaded ? "Attivo…" : "Scarico legendary…", async () => {
      const turnOn = !settings.epicLegendary;
      const saved = await api.legendaryEnable(turnOn);
      useApp.setState({ settings: saved });
      toast(
        turnOn
          ? "I giochi Epic ora passano da GameMode"
          : "I giochi Epic tornano all'Epic Games Launcher; quelli installati da GameMode ricompaiono se riattivi l'opzione",
      );
    });

  const login = () =>
    work("Apro la pagina di Epic…", async () => {
      await api.legendaryLogin();
      toast(
        settings.keyboardAssist
          ? "Accedi nella pagina di Epic. Col controller: A conferma, B annulla, Y tastiera, ⧉ torna a GameMode"
          : "Accedi nella pagina di Epic. Tieni premuto ⧉ per tornare qui",
      );
    });

  const importAuth = () =>
    work("Importo l'accesso…", async () => {
      const account = await api.legendaryImportAuth();
      toast(`Accesso a Epic come ${account}`);
    });

  const logout = () =>
    work("Esco…", async () => {
      await api.legendaryLogout();
      toast("Account Epic scollegato");
    });

  const importGames = () =>
    work("Preparo l'importazione…", async () => {
      await api.legendaryImportGames();
      toast("Importazione avviata: la vedi in Scarica");
    });

  const toggleHint = busy && !enabled
    ? download != null
      ? `Scarico legendary… ${Math.round(download * 100)}%`
      : busy
    : enabled
      ? "Installazioni, aggiornamenti e avvio dei giochi Epic passano da legendary"
      : "Installa, aggiorna e avvia i giochi Epic senza aprire l'Epic Games Launcher";

  return (
    <Section title="Epic Games">
      <Row
        label="Gestisci i giochi Epic senza Epic Games Launcher"
        hint={toggleHint}
        onClick={() => void toggle()}
        control={<Toggle on={settings.epicLegendary} />}
      />
      {enabled &&
        (status.account ? (
          <Row
            label={`Account Epic: ${status.account}`}
            hint={busy ?? "Esci per scollegare l'account da GameMode"}
            onClick={() => void logout()}
            control={<span className="setting-action">Esci</span>}
          />
        ) : (
          <>
            <Row
              label="Accedi a Epic Games"
              hint={busy ?? "Non collegato. Si apre la pagina di accesso di Epic"}
              onClick={() => void login()}
              control={<span className="setting-action">Accedi</span>}
            />
            {status.eglSignedIn && (
              <Row
                label="Usa l'accesso dell'Epic Games Launcher"
                hint="Prende la sessione già aperta su questo PC; l'Epic Games Launcher verrà disconnesso"
                onClick={() => void importAuth()}
              />
            )}
          </>
        ))}
      {enabled && status.account && status.eglGames > 0 && (
        <Row
          label={`Importa i giochi dell'Epic Games Launcher (${status.eglGames})`}
          hint="Li prende in gestione senza scaricarli di nuovo"
          onClick={() => void importGames()}
        />
      )}
      <Row
        label={`Usa legendary ${status.version}, software libero con licenza GPL-3.0`}
        hint="Scaricato da GitHub alla prima attivazione e verificato prima dell'uso"
        onClick={() => void api.openLink(status.licenseUrl)}
        control={<span className="setting-action">Licenza</span>}
      />
    </Section>
  );
}
