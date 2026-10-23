export type Store = "steam" | "xbox" | "epic" | "gog" | "ea" | "ubisoft" | "manual" | "emulator" | "mock" | "app";

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
  /** Space on disk; null while unknown. */
  sizeBytes: number | null;
  /** Unix seconds of the last folder walk; null when the store manifest gave the size. */
  sizeCheckedAt: number | null;
  /** Part of the folder couldn't be read: the size is a lower bound. */
  sizePartial: boolean;
  /** Store page facts; null until the lookup ran, or when nobody knows the game. */
  meta?: GameMeta | null;
  metaCheckedAt?: number | null;
}

/** Seconds, as IGDB reports them. */
export interface TimeToBeat {
  main: number | null;
  extra: number | null;
  complete: number | null;
}

export interface GameMeta {
  summary: string | null;
  genres: string[];
  developer: string | null;
  /** As the source prints it: "25 feb 2022" or "2021-11-09". */
  releaseDate: string | null;
  /** 0-100. */
  criticScore: number | null;
  scoreSource: "metacritic" | "igdb" | null;
  /** Local files, to pass through `artUrl`. */
  screenshots: string[];
  timeToBeat: TimeToBeat | null;
  igdbChecked: boolean;
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
  /** Freeze the game while Wand has the screen. */
  wandPause: boolean;
  /** Controller vibration for the interface: 0 off, 1 light, 2 normal. */
  haptics: number;
  /** Now playing and the per-app mixer in the overlay and quick menu. */
  musicControls: boolean;
  /** Volume keys and View + D-pad show the volume over the game. */
  osdVolume: boolean;
  /** Minutes of inactivity before the ambient screensaver; 0 = never. */
  idleScreensaverMin: number;
  /** Minutes of inactivity before the display turns off; 0 = never. */
  idleDisplayOffMin: number;
  /** Minutes of inactivity before `idleAction`; 0 = never. */
  idleSleepMin: number;
  idleAction: PowerAction;
  /** Inactivity never sleeps the PC while a game downloads or updates. */
  idleWaitDownloads: boolean;
  /** The system keyboard opens by itself on text fields in store windows. */
  keyboardAuto: boolean;
  /** When Windows shuts down with a game open, back up its saves once it has closed. */
  backupOnExit: boolean;
  /** Twitch app credentials for IGDB (game cards). */
  igdbClientId: string | null;
  igdbClientSecret: string | null;
  /** View+RB held during a game takes a screenshot. */
  captureCombo: boolean;
  /** Phone companion: the LAN remote server. */
  companionEnabled: boolean;
  /** DualSense, DualShock 4 and Switch Pro read directly over HID. */
  hidPads: boolean;
  /** In console mode the Xbox button opens GameMode instead of Game Bar. */
  guideToGamemode: boolean;
}

export interface ShellStatus {
  /** GameMode is this user's Windows shell. */
  enabled: boolean;
  /** This session started in console mode (no Explorer). */
  active: boolean;
  /** This GameMode was restarted by an update. */
  afterUpdate: boolean;
  /** Console mode is on, but this sign-in landed on the Windows desktop once. */
  desktopOnce: boolean;
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

export type PadKind = "xbox" | "playstation" | "nintendo";

export interface Controller {
  slot: number;
  wired: boolean;
  /** 0 empty … 3 full. */
  level: number;
  /** Button layout, for the glyphs. */
  kind: PadKind;
  /** "Controller 1", "DualSense", "Pro Controller"… */
  name: string;
}

export interface ManualGame {
  id: string;
  name: string;
  path: string;
  args: string[];
}

export type Phase = "launching" | "running" | "suspended" | "exited" | "failed";

export interface SessionState {
  gameId: string;
  /** Kept with the session, so a frozen game shows even when the library lost it. */
  gameName: string;
  phase: Phase;
  startedAt: number;
  seconds: number;
  /** The game is being updated before it can start. */
  update?: GameUpdate | null;
  /** Unix seconds since when the game is frozen (Quick Resume). */
  suspendedAt?: number | null;
  /** An anti-cheat was found: suspending may get the player kicked. */
  antiCheat?: boolean;
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

/** `audio://volume`: pushed to every window when the system volume changes. */
export interface VolumeEvent extends Volume {
  /** Changed from GameMode's own quick menu or overlay. */
  own: boolean;
}

/** `osd://show`: what the volume indicator window displays. */
export interface OsdPayload extends Volume {
  /** Shown instead of "Volume": the new output after a device change. */
  label: string | null;
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
  /** Download size, when the catalog says (Game Pass). */
  sizeBytes?: number | null;
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

export type PowerAction = "shutdown" | "restart" | "sleep" | "restartToDesktop" | "hibernate" | "lock" | "logoff" | "updateRestart" | "updateShutdown";

/** Trucchi: what a memory value is stored as. */
export type CheatKind = "int" | "float" | "double";

export interface CheatHit {
  address: number;
  value: string;
  frozen: boolean;
}

export interface CheatScan {
  kind: CheatKind;
  count: number;
  /** Listed once the search is down to a handful. */
  hits: CheatHit[];
}

export interface CheatsStatus {
  available: boolean;
  /** Why not, when not available. */
  reason: string | null;
  scan: CheatScan | null;
  frozen: CheatHit[];
}

export type NetworkKind = "none" | "wifi" | "ethernet" | "cellular";

export interface NetworkInfo {
  kind: NetworkKind;
  /** Wi-Fi signal, 0–4; null on Ethernet or when unknown. */
  bars: number | null;
  ssid: string | null;
  /** The internet is reachable, not just the local network. */
  internet: boolean;
}

export interface BatteryInfo {
  /** 0–100. */
  percent: number;
  charging: boolean;
  /** On mains power (charging or already full). */
  plugged: boolean;
  /** Windows' battery saver is on. */
  saver: boolean;
}

export interface BluetoothInfo {
  /** null when the PC has no Bluetooth radio. */
  on: boolean | null;
  /** Devices connected right now. */
  connected: number;
}

/** The top bar's system status: `system://status` events, `getSystemStatus` for the first paint. */
export interface SystemInfo {
  network: NetworkInfo;
  /** null on a desktop without a battery. */
  battery: BatteryInfo | null;
  bluetooth: BluetoothInfo;
}

/** What the system's media session (Spotify, a browser…) is playing. */
export interface NowPlaying {
  title: string;
  artist: string;
  album: string;
  playing: boolean;
  /** Readable name of the player ("Spotify", "Edge"). */
  source: string;
  /** Lowercase executable stem of the player, to find it in the mixer. */
  sourceExe: string;
  /** `data:` URL of the cover art, when the player publishes one. */
  thumbnail: string | null;
}

export type MediaAction = "play" | "pause" | "toggle" | "next" | "prev";

export type AppKind = "game" | "music" | "chat" | "browser" | "other";

/** One row of the per-app mixer; `level` is relative to the master volume. */
export interface AppVolume {
  key: string;
  label: string;
  level: number;
  muted: boolean;
  kind: AppKind;
  active: boolean;
}

export type IdleStage = "active" | "screensaver" | "displayOff";

export type PowerMode = { kind: "in"; secs: number } | { kind: "atDownloadsEnd" };

export interface PowerSchedule {
  action: PowerAction;
  /** Unix seconds when it happens; null while waiting for the downloads to end. */
  at: number | null;
  atDownloadsEnd: boolean;
}

export interface PowerCountdown {
  action: PowerAction;
  secondsLeft: number;
}

/** Keys the system keyboard sends besides text. */
export type TypedKey = "enter" | "backspace" | "escape" | "tab" | "space";

export interface Drive {
  /** `C:\` on Windows. */
  mount: string;
  label: string;
  total: number;
  available: number;
  removable: boolean;
  /** Bytes of the library's games on this drive (known sizes only). */
  gamesBytes: number;
  games: number;
}

export interface StorageOverview {
  drives: Drive[];
  /** Sizes are still being measured in the background. */
  measuring: boolean;
  /** Installed games whose size isn't known yet: a measure is pending. */
  unknown: number;
  /** Games measured that came out without a size (protected or shared folders). */
  unreadable: number;
  /** Where the Xbox app installs, as far as GameMode can tell. */
  xboxMount: string | null;
}

export interface Cleanup {
  artworkItems: number;
  artworkBytes: number;
  backupItems: number;
  backupBytes: number;
  applied: boolean;
}

/** Which power actions this PC supports. */
export interface PowerOptions {
  sleep: boolean;
  hibernate: boolean;
}

/** What the PC is doing: sleep, wake, lock screen, end of the session. */
export type PowerEvent =
  | { kind: "suspended" }
  | { kind: "resumed"; slept: number }
  | { kind: "locked" }
  | { kind: "unlocked" }
  | { kind: "displayOff" }
  | { kind: "displayOn" }
  | { kind: "sessionEnding" };

/** Windows Update, as far as a non-elevated shell can see. */
export type SystemUpdateState = "none" | "available" | "ready" | "rebootRequired" | "unknown";

export interface SystemUpdateItem {
  title: string;
  /** Download size in bytes, 0 when Windows doesn't say. */
  bytes: number;
  downloaded: boolean;
}

export interface SystemUpdates {
  state: SystemUpdateState;
  items: SystemUpdateItem[];
  /** Unix seconds of the last answer from Windows Update. */
  checkedAt: number | null;
  /** Why the state is `unknown`. */
  error: string | null;
}

export interface CrashRecord {
  /** Unix seconds when the watchdog saw GameMode die. */
  at: number;
  exitCode: number;
  /** The watchdog closed GameMode because its window stopped answering. */
  hung: boolean;
}

/** How this GameMode came up (normal start, restarted after a crash, safe mode). */
export interface StartupInfo {
  /** Crashes in a row the watchdog recovered from before this start (0: a normal start). */
  recovered: number;
  safeMode: boolean;
  lastCrash: CrashRecord | null;
  lastCrashDescription: string | null;
  /** First lines of the last panic (message and location). */
  lastPanic: string | null;
  crashCount: number;
  logPath: string;
}

export interface Capture {
  gameId: string;
  gameName: string;
  path: string;
  /** Unix seconds. */
  takenAt: number;
}

/** What the in-game card shows after a screenshot. */
export type CaptureNotice = { kind: "saved"; capture: Capture } | { kind: "error"; message: string };

export interface CompanionStatus {
  enabled: boolean;
  running: boolean;
  /** `http://<lan ip>:<port>`, to type by hand. */
  address: string | null;
  /** The same with the token: what the QR code encodes. */
  url: string | null;
  qrSvg: string | null;
  pin: string | null;
  firewall: "ok" | "missing" | "unknown";
  error: string | null;
}

/** Text sent from the phone's keyboard (`remote://text`). */
export interface RemoteText {
  text: string;
  enter: boolean;
  backspace: boolean;
}
