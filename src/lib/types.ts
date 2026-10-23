export type Store = "steam" | "xbox" | "epic" | "gog" | "ea" | "ubisoft" | "manual" | "emulator" | "mock";

export interface Artwork {
  cover?: string | null;
  hero?: string | null;
  logo?: string | null;
}

export interface Game {
  id: string;
  store: Store;
  name: string;
  installDir: string | null;
  steamAppid: number | null;
  artwork: Artwork;
  artworkCheckedAt: number | null;
  favorite: boolean;
  hidden: boolean;
  addedAt: number;
  lastPlayed: number | null;
  playtimeSecs: number;
}

export interface Settings {
  steamgriddbKey: string | null;
  disabledStores: Store[];
  startFullscreen: boolean;
  preloadSteam: boolean;
  steamApiKey: string | null;
  uiSounds: boolean;
  tvMode: DisplayMode | null;
  tvAudio: string | null;
  performanceMode: boolean;
  closeApps: string[];
  overlayEnabled: boolean;
  backupDir: string | null;
  keyboardAssist: boolean;
  steamBigPicture: boolean;
  /** In Steam games Guide is left to Steam; Back+Start opens GameMode's overlay. */
  steamGuide: boolean;
  /** Wand's overlay shortcut is Alt+` instead of Alt+W. */
  wandBacktick: boolean;
  /** LB+RB held during a game opens Wand's overlay. */
  wandCombo: boolean;
}

export interface ShellStatus {
  /** GameMode is this user's Windows shell. */
  enabled: boolean;
  /** This session started in console mode (no Explorer). */
  active: boolean;
}

export interface UpdateInfo {
  version: string;
  notes: string | null;
}

export interface BackupInfo {
  id: string;
  created: number;
  automatic: boolean;
  bytes: number;
}

export interface SaveInfo {
  known: boolean;
  locations: string[];
  backups: BackupInfo[];
  backupDir: string;
}

export interface DisplayMode {
  width: number;
  height: number;
  refresh: number;
}

export interface DisplayInfo {
  modes: DisplayMode[];
  current: DisplayMode | null;
}

export interface AudioOutput {
  id: string;
  name: string;
  default: boolean;
}

export interface Radios {
  wifi: boolean | null;
  bluetooth: boolean | null;
}

export interface WifiNetwork {
  ssid: string;
  bars: number;
  secured: boolean;
  connected: boolean;
}

export interface BtDevice {
  id: string;
  name: string;
  paired: boolean;
  connected: boolean;
}

export interface NewsItem {
  title: string;
  url: string;
  date: number;
  snippet: string;
}

export interface Achievement {
  name: string;
  description: string;
  icon: string | null;
  achieved: boolean;
  unlockTime: number;
}

export interface SteamExtras {
  news: NewsItem[];
  achievements: Achievement[];
  achievementStatus: "ok" | "needsKey" | "none" | "unavailable";
}

export interface Deal {
  appid: number;
  name: string;
  discount: number;
  initial: string;
  price: string;
  cover: string;
  hero: string;
}

export interface Deals {
  items: Deal[];
  status: "ok" | "noAccount" | "empty" | "failed";
}

export interface Session {
  gameId: string;
  profileId: string;
  /** Unix seconds. */
  start: number;
  end: number;
  seconds: number;
}

export interface SystemStatus {
  key: string;
  name: string;
  path: string | null;
  core: string;
  coreInstalled: boolean;
  roms: number;
}

export interface EmulatorStatus {
  retroarch: string | null;
  detected: boolean;
  systems: SystemStatus[];
}

export interface Profile {
  id: string;
  name: string;
  color: string;
}

export interface Profiles {
  profiles: Profile[];
  active: string;
}

export interface Collection {
  id: string;
  name: string;
  gameIds: string[];
}

export interface Controller {
  slot: number;
  wired: boolean;
  /** 0 empty … 3 full. */
  level: number;
}

export interface ManualGame {
  id: string;
  name: string;
  path: string;
  args: string[];
}

export type Phase = "launching" | "running" | "exited" | "failed";

export interface SessionState {
  gameId: string;
  phase: Phase;
  startedAt: number;
  seconds: number;
  /** The game is being updated before it can start. */
  update?: GameUpdate | null;
}

export type UpdateState = "available" | "downloading" | "installing" | "paused";

export interface GameUpdate {
  gameId: string;
  state: UpdateState;
  /** Bytes done and in total; 0 when the store doesn't say. */
  done: number;
  total: number;
  /** 0…1, when known. */
  progress: number | null;
}

export interface Volume {
  level: number;
  muted: boolean;
}

export interface ScanError {
  store: Store;
  message: string;
}

export interface LibraryUpdate {
  games: Game[];
  errors: ScanError[];
}

export type Action =
  | "up"
  | "down"
  | "left"
  | "right"
  | "confirm"
  | "back"
  | "menu"
  | "view"
  | "x"
  | "y"
  | "lb"
  | "rb"
  | "lt"
  | "rt"
  | "guide";

export type Direction = "up" | "down" | "left" | "right";

export interface Storefront {
  store: Store;
  opensIn: "client" | "browser";
  clientInstalled: boolean;
}

export interface CatalogItem {
  /** `<source>:<store id>`, e.g. `steam:570` or `gamepass:9NKX70BBCDRN`. */
  id: string;
  source: "steam" | "gamepass" | "epic" | "gog" | "cloud" | "deal";
  store: Store;
  name: string;
  cover: string | null;
  hero: string | null;
  playtimeSecs: number;
  /** Wishlist deals only. */
  discount?: number;
  price?: string;
  initial?: string;
}

export type SteamCatalogStatus = "ok" | "notInstalled" | "needsKey" | "noAccount" | "failed";

export interface Catalog {
  items: CatalogItem[];
  steam: SteamCatalogStatus;
  gamepassOk: boolean;
}

export type UninstallOutcome = "started" | "removed";

export type PowerAction = "shutdown" | "restart" | "sleep";
