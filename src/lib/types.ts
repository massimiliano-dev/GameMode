export type Store = "steam" | "xbox" | "epic" | "gog" | "ea" | "ubisoft" | "manual" | "mock";

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

export type PowerAction = "shutdown" | "restart" | "sleep";
