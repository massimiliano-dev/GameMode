import { useSyncExternalStore } from "react";

import { useApp } from "../store/app";
import { api, on } from "./api";

// Whether the backend is looking up game cards right now, so a game without
// a card says "searching" only while a search actually runs.
let running = false;
let watching = false;
const subscribers = new Set<() => void>();

function setRunning(value: boolean) {
  running = value;
  subscribers.forEach((notify) => notify());
}

function watch() {
  if (watching) return;
  watching = true;
  let heard = false;
  on<boolean>("metadata://running", (value) => {
    heard = true;
    setRunning(value);
  });
  on<null>("metadata://igdb-rejected", () =>
    useApp.getState().toast("Twitch ha rifiutato le chiavi IGDB: controllale in Impostazioni → Librerie"),
  );
  void api
    .metadataRunning()
    .then((value) => !heard && setRunning(!!value))
    .catch(() => {});
}

function subscribe(notify: () => void) {
  subscribers.add(notify);
  return () => void subscribers.delete(notify);
}

export function useMetadataRunning(): boolean {
  watch();
  return useSyncExternalStore(subscribe, () => running);
}
