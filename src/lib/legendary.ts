import { formatBytes } from "./format";
import type { Game, LegendaryJob } from "./types";

/** An Epic game installed and launched through legendary (not the Epic Games Launcher). */
export function isLegendary(game: Game): boolean {
  const launch = (game as Game & { launch?: { kind?: string; path?: string } }).launch;
  return game.store === "epic" && launch?.kind === "exe" && /legendary\.exe$/i.test(launch.path ?? "");
}

const KIND_LABEL: Record<LegendaryJob["kind"], string> = {
  install: "Installazione",
  update: "Aggiornamento",
  repair: "Riparazione",
  uninstall: "Disinstallazione",
  import: "Importazione",
};

export function jobActive(job: LegendaryJob): boolean {
  return job.state === "queued" || job.state === "running";
}

function formatEta(secs: number): string {
  if (secs < 60) return "meno di un minuto";
  const minutes = Math.round(secs / 60);
  if (minutes < 60) return `${minutes} min`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** "Installazione · 45% · 1,2 GB di 4,5 GB · ancora 12 min". */
export function describeJob(job: LegendaryJob): string {
  const kind = KIND_LABEL[job.kind];
  switch (job.state) {
    case "queued":
      return `${kind} in coda`;
    case "done":
      return `${kind} completata`;
    case "cancelled":
      return `${kind} annullata`;
    case "failed":
      return job.error ?? `${kind} non riuscita`;
  }
  const parts = [job.phase === "verifying" ? "Verifica dei file" : job.phase === "preparing" ? `${kind}: preparazione` : kind];
  if (job.progress != null) parts.push(`${Math.round(job.progress * 100)}%`);
  if (job.total > 0 && job.phase === "downloading") parts.push(`${formatBytes(job.done)} di ${formatBytes(job.total)}`);
  if (job.etaSecs) parts.push(`ancora ${formatEta(job.etaSecs)}`);
  return parts.join(" · ");
}
