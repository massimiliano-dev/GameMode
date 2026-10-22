import { create } from "zustand";

import { currentFocusId } from "../input/focus";
import { api, on } from "../lib/api";
import { formatPlaytime } from "../lib/format";
import type { Game, LibraryUpdate, ScanError, SessionState, Settings, Store } from "../lib/types";

export type Tab = "home" | "library" | "downloads" | "stores" | "settings";
export type Screen = { name: Tab } | { name: "detail"; gameId: string };

interface Entry {
  screen: Screen;
  /** Focus to restore when coming back to this screen. */
  focusId?: string;
}

export interface Toast {
  id: number;
  text: string;
  tone: "info" | "error";
}

export interface Hint {
  button: "A" | "B" | "X" | "Y" | "LB" | "RB" | "LT" | "RT" | "menu" | "view";
  /** Second button sharing the same label, e.g. LT/RT. */
  alt?: Hint["button"];
  label: string;
}

interface AppState {
  games: Game[];
  loaded: boolean;
  scanning: boolean;
  scanErrors: ScanError[];
  settings: Settings | null;
  stack: Entry[];
  quickMenu: boolean;
  session: SessionState | null;
  /** Game whose art fills the background. */
  backdropId: string | null;
  /** Art for things that aren't installed games (catalog entries). */
  backdropArt: BackdropArt | null;
  hints: Hint[];
  toasts: Toast[];
  /** A shop was opened: rescan when the user comes back, new games may be installed. */
  rescanOnReturn: boolean;

  init: () => Promise<void>;
  rescan: () => Promise<void>;
  setTab: (tab: Tab) => void;
  open: (screen: Screen) => void;
  back: () => boolean;
  setQuickMenu: (open: boolean) => void;
  setBackdrop: (id: string | null) => void;
  setBackdropArt: (art: BackdropArt | null) => void;
  setHints: (hints: Hint[]) => void;
  upsertGame: (game: Game) => void;
  saveSettings: (settings: Settings) => Promise<void>;
  toggleFavorite: (game: Game) => Promise<void>;
  toggleHidden: (game: Game) => Promise<void>;
  launch: (id: string) => Promise<void>;
  openStore: (store: Store, gameId?: string) => Promise<void>;
  installFromCatalog: (id: string, name: string) => Promise<void>;
  uninstall: (game: Game) => Promise<void>;
  returnedToApp: () => void;
  toast: (text: string, tone?: Toast["tone"]) => void;
}

let toastId = 0;
let initialized = false;

export const useApp = create<AppState>((set, get) => ({
  games: [],
  loaded: false,
  scanning: false,
  scanErrors: [],
  settings: null,
  stack: [{ screen: { name: "home" } }],
  quickMenu: false,
  session: null,
  backdropId: null,
  backdropArt: null,
  hints: [],
  toasts: [],
  rescanOnReturn: false,

  init: async () => {
    if (initialized) return;
    initialized = true;
    on<LibraryUpdate>("library://updated", (u) => set({ games: u.games, scanErrors: u.errors, loaded: true }));
    on<Game>("library://game", (g) => get().upsertGame(g));
    on<SessionState>("game://state", (s) => onSessionEvent(s, set, get));

    const [games, settings, session] = await Promise.all([
      api.getLibrary(),
      api.getSettings(),
      api.getSession(),
    ]);
    // An empty saved library means first run: wait for the initial scan.
    set({ games, settings, session, loaded: games.length > 0 });
    if (games.length === 0) setTimeout(() => set({ loaded: true }), 8000);
  },

  rescan: async () => {
    if (get().scanning) return;
    set({ scanning: true });
    try {
      const update = await api.rescan();
      set({ games: update.games, scanErrors: update.errors, loaded: true });
      get().toast(`Libreria aggiornata: ${update.games.length} giochi`);
    } catch (e) {
      get().toast(String(e), "error");
    } finally {
      set({ scanning: false });
    }
  },

  setTab: (tab) => {
    const top = get().stack.at(-1)!;
    if (top.screen.name === tab) return;
    set({ stack: [{ screen: { name: tab } }] });
  },

  open: (screen) => {
    const stack = [...get().stack];
    stack[stack.length - 1] = { ...stack[stack.length - 1], focusId: currentFocusId() };
    set({ stack: [...stack, { screen }] });
  },

  back: () => {
    const stack = get().stack;
    if (stack.length > 1) {
      set({ stack: stack.slice(0, -1) });
      return true;
    }
    if (stack[0].screen.name !== "home") {
      set({ stack: [{ screen: { name: "home" } }] });
      return true;
    }
    return false;
  },

  setQuickMenu: (quickMenu) => set({ quickMenu }),
  setBackdrop: (backdropId) => set({ backdropId, backdropArt: null }),
  setBackdropArt: (backdropArt) => set({ backdropArt, backdropId: null }),
  setHints: (hints) => set({ hints }),

  upsertGame: (game) =>
    set((s) => ({
      games: s.games.some((g) => g.id === game.id)
        ? s.games.map((g) => (g.id === game.id ? game : g))
        : [...s.games, game],
    })),

  saveSettings: async (settings) => {
    const saved = await api.saveSettings(settings);
    set({ settings: saved });
  },

  toggleFavorite: async (game) => {
    const updated = await api.setFavorite(game.id, !game.favorite);
    get().upsertGame(updated);
    get().toast(updated.favorite ? `${game.name} aggiunto ai preferiti` : `${game.name} rimosso dai preferiti`);
  },

  toggleHidden: async (game) => {
    const updated = await api.setHidden(game.id, !game.hidden);
    get().upsertGame(updated);
    get().toast(updated.hidden ? `${game.name} nascosto` : `${game.name} di nuovo visibile`);
  },

  launch: async (id) => {
    try {
      await api.launch(id);
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  openStore: async (store, gameId) => {
    try {
      const opensIn = await api.openStore(store, gameId);
      set({ rescanOnReturn: true });
      get().toast(
        opensIn === "client"
          ? "Negozio aperto nel client. Premi Guide per tornare qui"
          : "Negozio aperto nel browser. Premi Guide per tornare qui",
      );
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  installFromCatalog: async (id, name) => {
    try {
      await api.installCatalogItem(id);
      set({ rescanOnReturn: true });
      get().toast(`Installazione di ${name} aperta nel client. Premi Guide per tornare qui`);
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  uninstall: async (game) => {
    try {
      const outcome = await api.uninstallGame(game.id);
      if (outcome === "removed") {
        get().toast(`${game.name} disinstallato`);
        get().back();
      } else {
        set({ rescanOnReturn: true });
        get().toast(`Completa la disinstallazione di ${game.name}, poi premi Guide per tornare qui`);
      }
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  returnedToApp: () => {
    if (!get().rescanOnReturn) return;
    set({ rescanOnReturn: false });
    void get().rescan();
  },

  toast: (text, tone = "info") => {
    const id = ++toastId;
    set((s) => ({ toasts: [...s.toasts, { id, text, tone }] }));
    setTimeout(() => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })), 4500);
  },
}));

function onSessionEvent(
  s: SessionState,
  set: (partial: Partial<AppState>) => void,
  get: () => AppState,
) {
  const name = get().games.find((g) => g.id === s.gameId)?.name ?? "Il gioco";
  if (s.phase === "exited") {
    set({ session: null });
    if (s.seconds > 0) get().toast(`${name}: hai giocato ${formatPlaytime(s.seconds)}`);
  } else if (s.phase === "failed") {
    set({ session: null });
    get().toast(`${name} non si è avviato (o non è stato rilevato)`, "error");
  } else {
    set({ session: s });
  }
}

export interface BackdropArt {
  key: string;
  hero: string | null;
  cover: string | null;
  store: Store;
}

export const currentScreen = (s: AppState) => s.stack.at(-1)!;
export const visibleGames = (games: Game[]) => games.filter((g) => !g.hidden);
