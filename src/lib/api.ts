import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { mockInvoke, mockListen } from "./mock";
import type {
  Catalog,
  Game,
  LibraryUpdate,
  ManualGame,
  PowerAction,
  SessionState,
  Settings,
  Store,
  Storefront,
  UninstallOutcome,
  Volume,
} from "./types";

/** False when the UI runs in a plain browser (`npm run dev`), backed by mocks. */
export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return isTauri ? invoke<T>(cmd, args) : mockInvoke<T>(cmd, args);
}

export const api = {
  getLibrary: () => call<Game[]>("get_library"),
  rescan: () => call<LibraryUpdate>("rescan_library"),
  launch: (id: string) => call<void>("launch_game", { id }),
  setFavorite: (id: string, value: boolean) => call<Game>("set_favorite", { id, value }),
  setHidden: (id: string, value: boolean) => call<Game>("set_hidden", { id, value }),
  getSettings: () => call<Settings>("get_settings"),
  saveSettings: (settings: Settings) => call<Settings>("save_settings", { settings }),
  getManualGames: () => call<ManualGame[]>("get_manual_games"),
  addManualGame: (name: string, path: string, args: string[]) =>
    call<ManualGame>("add_manual_game", { name, path, args }),
  removeManualGame: (id: string) => call<void>("remove_manual_game", { id }),
  getSession: () => call<SessionState | null>("get_session"),
  resumeGame: () => call<boolean>("resume_game"),
  closeGame: () => call<boolean>("close_game"),
  power: (action: PowerAction) => call<void>("power", { action }),
  getVolume: () => call<Volume>("get_volume"),
  setVolume: (level: number) => call<void>("set_volume", { level }),
  setMuted: (muted: boolean) => call<void>("set_muted", { muted }),
  exitToDesktop: () => call<void>("exit_to_desktop"),
  toggleFullscreen: () => call<void>("toggle_fullscreen"),
  quit: () => call<void>("quit_app"),
  nativeGamepad: () => call<boolean>("native_gamepad"),
  getAutostart: () => call<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => call<void>("set_autostart", { enabled }),
  refreshArtwork: () => call<void>("refresh_artwork"),
  listStorefronts: () => call<Storefront[]>("list_storefronts"),
  openStore: (store: Store, gameId?: string) =>
    call<Storefront["opensIn"]>("open_store", { store, gameId: gameId ?? null }),
  getCatalog: (force = false) => call<Catalog>("get_catalog", { force }),
  installCatalogItem: (id: string) => call<void>("install_catalog_item", { id }),
  uninstallGame: (id: string) => call<UninstallOutcome>("uninstall_game", { id }),
};

export function on<T>(event: string, handler: (payload: T) => void): () => void {
  if (!isTauri) return mockListen(event, handler);
  const unlisten = listen<T>(event, (e) => handler(e.payload));
  return () => void unlisten.then((fn) => fn());
}

/** Turns a cached artwork path into something an <img> can load. */
export function artUrl(path: string | null | undefined, version?: number | null): string | undefined {
  if (!path) return undefined;
  if (/^https?:/.test(path)) return path;
  return `${convertFileSrc(path)}${version ? `?v=${version}` : ""}`;
}

/** Native file picker for adding executables by hand. */
export async function pickExecutable(): Promise<string | null> {
  if (!isTauri) return "/Applications/Emulatore.app/Contents/MacOS/emulatore";
  const { open } = await import("@tauri-apps/plugin-dialog");
  const isWindows = navigator.userAgent.includes("Windows");
  const picked = await open({
    multiple: false,
    directory: false,
    filters: isWindows ? [{ name: "Programmi", extensions: ["exe", "bat", "cmd", "lnk"] }] : undefined,
  });
  return typeof picked === "string" ? picked : null;
}
