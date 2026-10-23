import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

import { mockInvoke, mockListen } from "./mock";
import type {
  AudioOutput,
  BackupInfo,
  BtDevice,
  Catalog,
  Cleanup,
  CatalogItem,
  Collection,
  Controller,
  Deals,
  DisplayInfo,
  EmulatorStatus,
  Game,
  GameUpdate,
  LibraryUpdate,
  ManualGame,
  PowerAction,
  Radios,
  SaveInfo,
  ShellStatus,
  Profiles,
  Session,
  SessionState,
  Settings,
  Store,
  SteamExtras,
  Storefront,
  StorageOverview,
  UpdateInfo,
  UninstallOutcome,
  Volume,
  WifiNetwork,
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
  getSuspended: () => call<SessionState[]>("get_suspended"),
  suspendGame: () => call<void>("suspend_game"),
  resumeSuspended: (gameId: string) => call<void>("resume_suspended", { gameId }),
  closeSuspended: (gameId: string) => call<void>("close_suspended", { gameId }),
  resumeGame: () => call<boolean>("resume_game"),
  getGameUpdates: () => call<GameUpdate[]>("get_game_updates"),
  wandInstalled: () => call<boolean>("wand_installed"),
  openWand: () => call<void>("open_wand"),
  openWandApp: () => call<void>("open_wand_app"),
  closeGame: () => call<boolean>("close_game"),
  power: (action: PowerAction) => call<void>("power", { action }),
  getVolume: () => call<Volume>("get_volume"),
  setVolume: (level: number) => call<void>("set_volume", { level }),
  setMuted: (muted: boolean) => call<void>("set_muted", { muted }),
  exitToDesktop: () => call<void>("exit_to_desktop"),
  overlayHide: (resume: boolean) => call<void>("overlay_hide", { resume }),
  getConsoleMode: () => call<ShellStatus>("get_console_mode"),
  setConsoleMode: (enabled: boolean) => call<ShellStatus>("set_console_mode", { enabled }),
  overlayToLauncher: () => call<void>("overlay_to_launcher"),
  toggleFullscreen: () => call<void>("toggle_fullscreen"),
  quit: () => call<void>("quit_app"),
  displayModes: () => call<DisplayInfo>("display_modes"),
  audioOutputs: () => call<AudioOutput[]>("audio_outputs"),
  setAudioOutput: (id: string) => call<void>("set_audio_output", { id }),
  applyTvMode: () => call<void>("apply_tv_mode"),
  getRadios: () => call<Radios>("get_radios"),
  setRadio: (bluetooth: boolean, on: boolean) => call<void>("set_radio", { bluetooth, on }),
  wifiNetworks: () => call<WifiNetwork[]>("wifi_networks"),
  wifiConnect: (ssid: string, password: string | null) => call<void>("wifi_connect", { ssid, password }),
  bluetoothDevices: (discover: boolean) => call<BtDevice[]>("bluetooth_devices", { discover }),
  bluetoothPair: (id: string) => call<void>("bluetooth_pair", { id }),
  bluetoothUnpair: (id: string) => call<void>("bluetooth_unpair", { id }),
  nativeGamepad: () => call<boolean>("native_gamepad"),
  getControllers: () => call<Controller[]>("get_controllers"),
  getAutostart: () => call<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => call<void>("set_autostart", { enabled }),
  refreshArtwork: () => call<void>("refresh_artwork"),
  checkUpdate: () => call<UpdateInfo | null>("check_update"),
  installUpdate: () => call<void>("install_update"),
  appVersion: async () => (isTauri ? (await import("@tauri-apps/api/app")).getVersion() : "0.0.0-dev"),
  listStorefronts: () => call<Storefront[]>("list_storefronts"),
  openStore: (store: Store, gameId?: string) =>
    call<Storefront["opensIn"]>("open_store", { store, gameId: gameId ?? null }),
  getCatalog: (force = false) => call<Catalog>("get_catalog", { force }),
  installCatalogItem: (id: string) => call<void>("install_catalog_item", { id }),
  getCloudCatalog: (force = false) => call<CatalogItem[]>("get_cloud_catalog", { force }),
  emulatorStatus: () => call<EmulatorStatus>("emulator_status"),
  saveInfo: (gameId: string) => call<SaveInfo>("save_info", { gameId }),
  backupGame: (gameId: string) => call<BackupInfo>("backup_game", { gameId }),
  restoreBackup: (gameId: string, backupId: string) => call<void>("restore_backup", { gameId, backupId }),
  deleteBackup: (gameId: string, backupId: string) => call<void>("delete_backup", { gameId, backupId }),
  backupAll: () => call<{ saved: number; skipped: number }>("backup_all"),
  setRetroarchPath: (path: string | null) => call<EmulatorStatus>("set_retroarch_path", { path }),
  setRomDir: (system: string, path: string | null) => call<EmulatorStatus>("set_rom_dir", { system, path }),
  uninstallGame: (id: string) => call<UninstallOutcome>("uninstall_game", { id }),
  getProfiles: () => call<Profiles>("get_profiles"),
  addProfile: (name: string) => call<Profiles>("add_profile", { name }),
  renameProfile: (id: string, name: string) => call<Profiles>("rename_profile", { id, name }),
  removeProfile: (id: string) => call<Profiles>("remove_profile", { id }),
  switchProfile: (id: string) => call<Profiles>("switch_profile", { id }),
  getSessions: () => call<Session[]>("get_sessions"),
  steamExtras: (appid: number) => call<SteamExtras>("steam_extras", { appid }),
  wishlistDeals: (force = false) => call<Deals>("wishlist_deals", { force }),
  openLink: (url: string) => call<void>("open_link", { url }),
  getCollections: () => call<Collection[]>("get_collections"),
  addCollection: (name: string) => call<Collection>("add_collection", { name }),
  removeCollection: (id: string) => call<void>("remove_collection", { id }),
  setInCollection: (collectionId: string, gameId: string, value: boolean) =>
    call<Collection[]>("set_in_collection", { collectionId, gameId, value }),
  storageOverview: (measure = false) => call<StorageOverview>("storage_overview", { measure }),
  storageCleanup: (apply: boolean) => call<Cleanup>("storage_cleanup", { apply }),
  storageRemeasure: () => call<void>("storage_remeasure"),
};

/** Like `on`, but only for events sent to this window (controller input). */
export function onWindow<T>(event: string, handler: (payload: T) => void): () => void {
  if (!isTauri) return mockListen(event, handler);
  const unlisten = listen<T>(event, (e) => handler(e.payload), { target: getCurrentWebviewWindow().label });
  return () => void unlisten.then((fn) => fn());
}

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

/** Native folder picker (ROM folders). */
export async function pickFolder(): Promise<string | null> {
  if (!isTauri) return "/Users/demo/ROM";
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ multiple: false, directory: true });
  return typeof picked === "string" ? picked : null;
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
