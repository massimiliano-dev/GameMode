import type { Game, GameUpdate, Store, UpdateState } from "./types";

export const STORE_LABEL: Record<Store, string> = {
  steam: "Steam",
  xbox: "Xbox",
  epic: "Epic Games",
  gog: "GOG",
  ea: "EA",
  ubisoft: "Ubisoft",
  manual: "Aggiunti a mano",
  emulator: "Emulatori",
  mock: "Demo",
  app: "App",
};

/** Names of the shops, which differ from the library names for Xbox. */
export const SHOP_LABEL: Partial<Record<Store, string>> = {
  steam: "Steam",
  xbox: "Microsoft Store",
  epic: "Epic Games Store",
  gog: "GOG",
  ubisoft: "Ubisoft Store",
  ea: "EA",
};

/** Stores the user can toggle in settings, in display order. */
export const STORES: Store[] = ["steam", "xbox", "epic", "gog", "ea", "ubisoft", "emulator", "manual"];

export function formatBytes(bytes: number): string {
  const gb = bytes / 1024 ** 3;
  if (gb >= 1) return `${gb.toFixed(1).replace(".", ",")} GB`;
  return `${Math.max(1, Math.round(bytes / 1024 ** 2))} MB`;
}

export const UPDATE_LABEL: Record<UpdateState, string> = {
  available: "Aggiornamento disponibile",
  downloading: "Download dell'aggiornamento",
  installing: "Installazione dell'aggiornamento",
  paused: "Aggiornamento in pausa",
};

/** "45% · 1,2 GB di 4,5 GB", or null when the store gives no numbers. */
export function updateProgress(u: GameUpdate): string | null {
  const parts = [
    u.progress != null && u.state !== "available" ? `${Math.round(u.progress * 100)}%` : null,
    u.total > 0 ? `${formatBytes(u.done)} di ${formatBytes(u.total)}` : null,
  ];
  return parts.filter(Boolean).join(" · ") || null;
}

export function formatPlaytime(secs: number): string {
  if (secs < 60) return secs > 0 ? "meno di un minuto" : "mai giocato";
  const minutes = Math.round(secs / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours >= 10 || rest === 0) return `${hours} h`;
  return `${hours} h ${rest} min`;
}

const relative = new Intl.RelativeTimeFormat("it", { numeric: "auto" });
const dateFmt = new Intl.DateTimeFormat("it", { day: "numeric", month: "short", year: "numeric" });

export function formatLastPlayed(ts: number | null): string {
  if (!ts) return "mai";
  const days = Math.floor((startOfDay(Date.now()) - startOfDay(ts * 1000)) / 86_400_000);
  if (days < 7) return relative.format(-days, "day");
  return dateFmt.format(new Date(ts * 1000));
}

function startOfDay(ms: number) {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** Drive letter of a Windows path ("D:"), or the volume name of a macOS one. */
export function driveOf(path: string | null | undefined): string | null {
  if (!path) return null;
  const letter = /^([a-zA-Z]):[\\/]/.exec(path);
  if (letter) return `${letter[1].toUpperCase()}:`;
  const volume = /^\/Volumes\/([^/]+)/.exec(path);
  return volume ? volume[1] : null;
}

/** "48,2 GB su D:", "circa 48,2 GB" for a partial walk, null when unknown. */
export function formatGameSize(game: Pick<Game, "sizeBytes" | "sizePartial" | "installDir">): string | null {
  if (game.sizeBytes == null) return null;
  const drive = driveOf(game.installDir);
  return `${game.sizePartial ? "circa " : ""}${formatBytes(game.sizeBytes)}${drive ? ` su ${drive}` : ""}`;
}

export function formatClock(d: Date): string {
  return d.toLocaleTimeString("it", { hour: "2-digit", minute: "2-digit" });
}

export function formatDate(d: Date): string {
  return d.toLocaleDateString("it", { weekday: "long", day: "numeric", month: "long" });
}
