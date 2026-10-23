import { create } from "zustand";

import { currentFocusId } from "../input/focus";
import { setSoundsEnabled } from "../input/sfx";
import { api, on } from "../lib/api";
import { formatPlaytime } from "../lib/format";
import type {
  Collection,
  Controller,
  Game,
  GameUpdate,
  LibraryUpdate,
  PowerEvent,
  Profile,
  ScanError,
  SessionState,
  Settings,
  Store,
  UpdateInfo,
} from "../lib/types";

export type Tab = "home" | "library" | "downloads" | "stores" | "stats" | "settings";
export type Screen = { name: Tab } | { name: "detail"; gameId: string } | { name: "search" } | { name: "connections" };

const TABS: Tab[] = ["home", "library", "downloads", "stores", "stats", "settings"];

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
  /** Games frozen by Quick Resume, oldest first. */
  suspended: SessionState[];
  /** Game whose art fills the background. */
  backdropId: string | null;
  /** Art for things that aren't installed games (catalog entries). */
  backdropArt: BackdropArt | null;
  hints: Hint[];
  toasts: Toast[];
  /** A shop was opened: rescan when the user comes back, new games may be installed. */
  rescanOnReturn: boolean;
  /** Text field the on-screen keyboard is typing into. */
  keyboardField: HTMLInputElement | null;
  controllers: Controller[];
  profiles: Profile[];
  activeProfile: string;
  /** A newer signed release, found at startup or from Settings. */
  update: UpdateInfo | null;
  collections: Collection[];
  /** Game updates the stores report, by game id. */
  gameUpdates: Record<string, GameUpdate>;
  /** The PC just woke up with a game still running: Home offers to go back to it. */
  wokeAt: number | null;
  /** Seconds before the automatic sleep, while the warning is up. */
  sleepSoon: number | null;

  init: () => Promise<void>;
  refreshGameUpdates: () => Promise<void>;
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
  /** Quick Resume: freezes the running game and comes back here. */
  suspendGame: () => Promise<boolean>;
  resumeSuspended: (gameId: string) => Promise<void>;
  closeSuspended: (gameId: string) => Promise<void>;
  /** Closes GameMode; the saved screen is forgotten. */
  quit: () => Promise<void>;
  exitToDesktop: () => Promise<void>;
  openStore: (store: Store, gameId?: string) => Promise<void>;
  /** Steam's downloads, or the game's page in the Xbox app. */
  showGameUpdate: (game: Game) => Promise<void>;
  installFromCatalog: (id: string, name: string) => Promise<void>;
  uninstall: (game: Game) => Promise<void>;
  returnedToApp: () => void;
  toast: (text: string, tone?: Toast["tone"]) => void;
  setKeyboardField: (field: HTMLInputElement | null) => void;
  switchProfile: (id: string) => Promise<void>;
  setProfiles: (profiles: Profile[], active: string) => void;
  createCollection: (name: string) => Promise<Collection | null>;
  setInCollection: (collectionId: string, gameId: string, value: boolean) => Promise<void>;
  removeCollection: (id: string) => Promise<void>;
  /** Back to the game that survived the sleep. */
  resumeAfterWake: () => Promise<void>;
}

let toastId = 0;
/** The window had the focus after the wake: the "Bentornato" card was seen. */
let wokeSeen = false;
/** Time to read the controller hint before a store window takes over. */
const HANDOFF_DELAY = 2200;
/** How often to ask the stores about game updates. */
const GAME_UPDATES_EVERY = 20_000;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let initialized = false;
/** The overlay window shares this store but never owns the screen stack. */
const isOverlayWindow = typeof window !== "undefined" && new URLSearchParams(window.location.search).has("overlay");

export const useApp = create<AppState>((set, get) => ({
  games: [],
  loaded: false,
  scanning: false,
  scanErrors: [],
  settings: null,
  stack: [{ screen: { name: "home" } }],
  quickMenu: false,
  session: null,
  suspended: [],
  backdropId: null,
  backdropArt: null,
  hints: [],
  toasts: [],
  rescanOnReturn: false,
  keyboardField: null,
  controllers: [],
  profiles: [],
  activeProfile: "",
  update: null,
  collections: [],
  gameUpdates: {},
  wokeAt: null,
  sleepSoon: null,

  init: async () => {
    if (initialized) return;
    initialized = true;
    on<LibraryUpdate>("library://updated", (u) => set({ games: u.games, scanErrors: u.errors, loaded: true }));
    on<Game>("library://game", (g) => get().upsertGame(g));
    on<SessionState>("game://state", (s) => {
      onSessionEvent(s, set, get);
      // Keeps the saved screen fresh through a long game.
      persistStack(get().stack);
    });
    on<Controller[]>("gamepad://controllers", (list) => onControllers(list, set, get));
    on<string>("uninstall://done", (name) => {
      set({ rescanOnReturn: false });
      get().toast(`${name} disinstallato`);
    });
    on<string>("install://queued", () => {
      set({ rescanOnReturn: false });
      get().toast("Download avviato in Steam: lo trovi in Libreria quando ha finito");
    });
    on<PowerEvent>("power://state", (e) => onPowerEvent(e, set, get));
    // The "Bentornato" card is done once the user has seen it and left
    // GameMode (back to the game from anywhere, Alt+Tab included).
    window.addEventListener("focus", () => {
      if (get().wokeAt) wokeSeen = true;
    });
    window.addEventListener("blur", () => {
      if (get().wokeAt && wokeSeen) set({ wokeAt: null });
    });
    void api.getControllers().then((controllers) => set({ controllers })).catch(() => {});

    const [games, settings, session, suspended, profiles, collections, shell] = await Promise.all([
      api.getLibrary(),
      api.getSettings(),
      api.getSession(),
      api.getSuspended().catch(() => []),
      api.getProfiles(),
      api.getCollections(),
      api.getConsoleMode().catch(() => null),
    ]);
    set({ profiles: profiles.profiles, activeProfile: profiles.active, collections, suspended });
    // Back where the user was: after a quick restart (a crash) or an update.
    if (!isOverlayWindow) {
      const stack = loadStack(!!shell?.afterUpdate);
      if (stack) set({ stack });
      // Idle on one screen for long: still the screen to come back to.
      setInterval(() => persistStack(get().stack), STACK_REFRESH_EVERY);
    }
    // An empty saved library means first run: wait for the initial scan.
    setSoundsEnabled(settings.uiSounds);
    setTimeout(() => void announceDeals(get().toast), 10_000);
    setTimeout(() => {
      void api
        .checkUpdate()
        .then((update) => {
          if (!update) return;
          set({ update });
          get().toast(`È disponibile GameMode ${update.version}: lo installi da Impostazioni → Aggiornamenti`);
        })
        .catch(() => {});
    }, 20_000);
    set({ games, settings, session, loaded: games.length > 0 });
    void get().refreshGameUpdates();
    setInterval(() => void get().refreshGameUpdates(), GAME_UPDATES_EVERY);
    if (games.length === 0) setTimeout(() => set({ loaded: true }), 8000);
  },

  refreshGameUpdates: async () => {
    const list = await api.getGameUpdates().catch(() => null);
    if (list) set({ gameUpdates: Object.fromEntries(list.map((u) => [u.gameId, u])) });
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
    persistStack(get().stack);
  },

  open: (screen) => {
    const stack = [...get().stack];
    stack[stack.length - 1] = { ...stack[stack.length - 1], focusId: currentFocusId() };
    set({ stack: [...stack, { screen }] });
    persistStack(get().stack);
  },

  back: () => {
    const stack = get().stack;
    if (stack.length > 1) {
      set({ stack: stack.slice(0, -1) });
      persistStack(get().stack);
      return true;
    }
    if (stack[0].screen.name !== "home") {
      set({ stack: [{ screen: { name: "home" } }] });
      persistStack(get().stack);
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
    setSoundsEnabled(saved.uiSounds);
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

  suspendGame: async () => {
    try {
      await api.suspendGame();
      return true;
    } catch (e) {
      get().toast(String(e), "error");
      return false;
    }
  },

  resumeSuspended: async (gameId) => {
    try {
      await api.resumeSuspended(gameId);
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  closeSuspended: async (gameId) => {
    try {
      await api.closeSuspended(gameId);
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  quit: async () => {
    forgetStack();
    await api.quit();
  },

  exitToDesktop: async () => {
    forgetStack();
    await api.exitToDesktop();
  },

  openStore: async (store, gameId) => {
    try {
      const what = "Apro il negozio.";
      const native = store === "xbox" || (store === "steam" && !!get().settings?.steamBigPicture);
      get().toast(native ? nativeHint(what) : assistHint(get().settings, what));
      await sleep(HANDOFF_DELAY);
      await api.openStore(store, gameId);
      set({ rescanOnReturn: true });
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  showGameUpdate: async (game) => {
    if (game.store !== "steam") return get().openStore(game.store, game.id);
    try {
      const what = "Apro i download di Steam.";
      get().toast(get().settings?.steamBigPicture ? nativeHint(what) : assistHint(get().settings, what));
      await sleep(HANDOFF_DELAY);
      await api.openLink("steam://open/downloads");
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  installFromCatalog: async (id, name) => {
    try {
      if (!id.startsWith("cloud:")) {
        // Shown while GameMode is still on screen: it steps aside next.
        const what = `Apro l'installazione di ${name}.`;
        const native = id.startsWith("gamepass:") || (id.startsWith("steam:") && !!get().settings?.steamBigPicture);
        get().toast(native ? nativeHint(what) : assistHint(get().settings, what));
        await sleep(HANDOFF_DELAY);
      }
      await api.installCatalogItem(id);
      set({ rescanOnReturn: true });
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  uninstall: async (game) => {
    try {
      if (!["manual", "xbox", "mock"].includes(game.store)) {
        const what = `Apro la disinstallazione di ${game.name}.`;
        get().toast(game.store === "steam" && get().settings?.steamBigPicture ? nativeHint(what) : assistHint(get().settings, what));
        await sleep(HANDOFF_DELAY);
      }
      const outcome = await api.uninstallGame(game.id);
      if (outcome === "removed") {
        get().toast(`${game.name} disinstallato`);
        get().back();
      } else {
        set({ rescanOnReturn: true });
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

  setKeyboardField: (keyboardField) => set({ keyboardField }),

  switchProfile: async (id) => {
    if (id === get().activeProfile) return;
    try {
      const p = await api.switchProfile(id);
      const collections = await api.getCollections();
      set({ profiles: p.profiles, activeProfile: p.active, collections });
      const name = p.profiles.find((x) => x.id === p.active)?.name;
      get().toast(`Ciao ${name}!`);
    } catch (e) {
      get().toast(String(e), "error");
    }
  },

  setProfiles: (profiles, activeProfile) => set({ profiles, activeProfile }),

  createCollection: async (name) => {
    try {
      const c = await api.addCollection(name);
      set({ collections: [...get().collections, c] });
      return c;
    } catch (e) {
      get().toast(String(e), "error");
      return null;
    }
  },

  setInCollection: async (collectionId, gameId, value) => {
    set({ collections: await api.setInCollection(collectionId, gameId, value) });
  },

  resumeAfterWake: async () => {
    set({ wokeAt: null });
    if (!get().session) return;
    if (!(await api.resumeGame().catch(() => false))) get().toast("Finestra del gioco non trovata");
  },

  removeCollection: async (id) => {
    await api.removeCollection(id);
    set({ collections: get().collections.filter((c) => c.id !== id) });
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
  const others = get().suspended.filter((p) => p.gameId !== s.gameId);
  // Only the game in the foreground owns `session`: a frozen one that ends
  // in the background leaves it alone.
  const mine = get().session?.gameId === s.gameId;
  if (s.phase === "exited") {
    set({ suspended: others, ...(mine ? { session: null } : {}) });
    if (s.seconds > 0) get().toast(`${name}: hai giocato ${formatPlaytime(s.seconds)}`);
  } else if (s.phase === "failed") {
    set({ suspended: others, ...(mine ? { session: null } : {}) });
    get().toast(`${name} non si è avviato (o non è stato rilevato)`, "error");
  } else if (s.phase === "suspended") {
    set({ suspended: [...others, s], ...(mine ? { session: null } : {}) });
    get().toast(`${name} in pausa: lo riprendi dalla Home`);
  } else {
    set({ session: s, suspended: others });
    return;
  }
  // No game to go back to after the wake.
  if (mine) set({ wokeAt: null });
}

/** Sleep, wake and the lock screen, as the backend reports them. */
function onPowerEvent(e: PowerEvent, set: (partial: Partial<AppState>) => void, get: () => AppState) {
  switch (e.kind) {
    case "suspended":
    case "locked":
    case "sessionEnding":
      set({ sleepSoon: null });
      break;
    case "resumed":
      // The game may have died in its sleep: the session is read again.
      void Promise.all([api.getSession(), api.getSuspended().catch(() => get().suspended)]).then(([session, suspended]) => {
        wokeSeen = document.hasFocus();
        set({ session, suspended, wokeAt: session ? Date.now() : null, sleepSoon: null });
        get().toast(session ? "Bentornato: la partita ti aspetta" : "Bentornato");
      });
      break;
    case "sleepSoon":
      set({ sleepSoon: e.seconds });
      break;
    case "sleepCancelled":
      set({ sleepSoon: null });
      break;
    default:
      break;
  }
}

/* ---------- Where the user was ---------- */

const STACK_KEY = "gm-stack";
/** A stack older than this is a fresh start, not a comeback after a crash. */
const STACK_MAX_AGE = 15 * 60_000;
const STACK_SAVE_DELAY = 300;
/** How often the saved stack is refreshed while GameMode sits on a screen. */
const STACK_REFRESH_EVERY = 60_000;
let stackSaveTimer: ReturnType<typeof setTimeout> | undefined;

/** Saves the screen stack (debounced) so a restart lands on the same screen. */
function persistStack(stack: Entry[]) {
  if (isOverlayWindow) return;
  clearTimeout(stackSaveTimer);
  stackSaveTimer = setTimeout(() => {
    // The top screen remembers its focus too, so a restart lands on the same card.
    const top = { ...stack[stack.length - 1], focusId: currentFocusId() ?? stack[stack.length - 1]?.focusId };
    try {
      localStorage.setItem(STACK_KEY, JSON.stringify({ stack: [...stack.slice(0, -1), top], savedAt: Date.now() }));
    } catch {
      // Storage unavailable: the next start begins at Home.
    }
  }, STACK_SAVE_DELAY);
}

function forgetStack() {
  clearTimeout(stackSaveTimer);
  try {
    localStorage.removeItem(STACK_KEY);
  } catch {
    // ignore
  }
}

/** The saved stack, when it is recent (or the restart was for an update) and well formed. */
export function loadStack(afterUpdate: boolean, now = Date.now()): Entry[] | null {
  let raw: string | null = null;
  try {
    raw = localStorage.getItem(STACK_KEY);
  } catch {
    return null;
  }
  if (!raw) return null;
  try {
    const saved = JSON.parse(raw) as { stack?: unknown; savedAt?: unknown };
    if (typeof saved.savedAt !== "number" || (!afterUpdate && now - saved.savedAt > STACK_MAX_AGE)) return null;
    const stack = Array.isArray(saved.stack) ? saved.stack.filter(isEntry) : [];
    if (!stack.length || stack[0].screen.name === "detail") return null;
    return stack;
  } catch {
    return null;
  }
}

function isEntry(value: unknown): value is Entry {
  if (!value || typeof value !== "object") return false;
  const { screen, focusId } = value as { screen?: { name?: unknown; gameId?: unknown }; focusId?: unknown };
  if (!screen || typeof screen !== "object" || typeof screen.name !== "string") return false;
  if (focusId !== undefined && typeof focusId !== "string") return false;
  if (screen.name === "detail") return typeof screen.gameId === "string";
  return (TABS as string[]).includes(screen.name) || screen.name === "search" || screen.name === "connections";
}

/** How to drive a store window GameMode just opened. */
function assistHint(settings: Settings | null, what: string) {
  return settings?.keyboardAssist
    ? `${what} Col controller: A conferma, B annulla, croce per muoverti, ⧉ torna a GameMode`
    : `${what} Tieni premuto ⧉ per tornare qui`;
}

/** For apps that already work with a controller (Xbox app, Store, Steam Big Picture). */
function nativeHint(what: string) {
  return `${what} Usa il controller come al solito; tieni premuto ⧉ per tornare a GameMode`;
}

/** Mentions discounted wishlist games, at most once a day. */
async function announceDeals(toast: (text: string) => void) {
  const today = new Date().toDateString();
  try {
    if (localStorage.getItem("gm-deals-announced") === today) return;
  } catch {
    // Storage unavailable: announcing again is harmless.
  }
  const deals = await api.wishlistDeals(false).catch(() => null);
  const n = deals?.items.length ?? 0;
  if (!n) return;
  toast(n === 1 ? "Un gioco della tua lista desideri Steam è in offerta: lo trovi in Scarica" : `${n} giochi della tua lista desideri Steam sono in offerta: li trovi in Scarica`);
  try {
    localStorage.setItem("gm-deals-announced", today);
  } catch {
    // ignore
  }
}

/** Warns once when a wireless controller's battery runs low. */
function onControllers(list: Controller[], set: (partial: Partial<AppState>) => void, get: () => AppState) {
  const before = get().controllers;
  for (const c of list) {
    const prev = before.find((p) => p.slot === c.slot);
    if (!c.wired && c.level <= 1 && (!prev || prev.wired || prev.level > 1)) {
      get().toast(`Batteria del controller ${c.slot + 1} quasi scarica`, "error");
    }
  }
  set({ controllers: list });
}

export interface BackdropArt {
  key: string;
  hero: string | null;
  cover: string | null;
  store: Store;
}

export const activeProfileOf = (s: AppState) => s.profiles.find((p) => p.id === s.activeProfile);

export const currentScreen = (s: AppState) => s.stack.at(-1)!;
export const visibleGames = (games: Game[]) => games.filter((g) => !g.hidden);
