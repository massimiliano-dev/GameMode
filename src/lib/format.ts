import type { Store } from "./types";

export const STORE_LABEL: Record<Store, string> = {
  steam: "Steam",
  xbox: "Xbox",
  epic: "Epic Games",
  gog: "GOG",
  ea: "EA",
  ubisoft: "Ubisoft",
  manual: "Aggiunti a mano",
  mock: "Demo",
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
export const STORES: Store[] = ["steam", "xbox", "epic", "gog", "ea", "ubisoft", "manual"];

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

export function formatClock(d: Date): string {
  return d.toLocaleTimeString("it", { hour: "2-digit", minute: "2-digit" });
}

export function formatDate(d: Date): string {
  return d.toLocaleDateString("it", { weekday: "long", day: "numeric", month: "long" });
}
