// In-browser stand-in for the Rust backend, so the UI can be developed and
// tested with `npm run dev` in any browser.

import type { Game, ManualGame, SessionState, Settings, Store, Volume } from "./types";

const CDN = "https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps";

const SEED: [Store, string, number][] = [
  ["steam", "Elden Ring", 1245620],
  ["steam", "Baldur's Gate 3", 1086940],
  ["steam", "Hades II", 1145350],
  ["steam", "Hollow Knight", 367520],
  ["steam", "Stardew Valley", 413150],
  ["steam", "Red Dead Redemption 2", 1174180],
  ["xbox", "Forza Horizon 5", 1551360],
  ["xbox", "Halo: The Master Chief Collection", 976730],
  ["xbox", "Starfield", 1716740],
  ["epic", "Alan Wake 2", 0],
  ["epic", "Death Stranding", 1190460],
  ["gog", "The Witcher 3: Wild Hunt", 292030],
  ["gog", "Cyberpunk 2077", 1091500],
  ["ubisoft", "Assassin's Creed Valhalla", 2208920],
  ["ea", "Apex Legends", 1172470],
  ["ea", "It Takes Two", 1426210],
];

const now = () => Math.floor(Date.now() / 1000);
const day = 86400;

let games: Game[] = SEED.map(([store, name, appid], i) => ({
  id: `${store}:mock-${i}`,
  store,
  name,
  installDir: null,
  steamAppid: appid || null,
  artwork: appid
    ? {
        cover: `${CDN}/${appid}/library_600x900_2x.jpg`,
        hero: `${CDN}/${appid}/library_hero.jpg`,
        logo: `${CDN}/${appid}/logo.png`,
      }
    : {},
  artworkCheckedAt: now(),
  favorite: i % 5 === 1,
  hidden: false,
  addedAt: now() - (i < 3 ? day : 30 * day),
  lastPlayed: i % 3 === 0 ? now() - i * day * 0.7 : null,
  playtimeSecs: i % 3 === 0 ? (i + 1) * 5400 : 0,
}));

let settings: Settings = { steamgriddbKey: null, disabledStores: [], startFullscreen: true, preloadSteam: true, steamApiKey: null };
let manual: ManualGame[] = [];
let session: SessionState | null = null;
let volume: Volume = { level: 0.6, muted: false };
let autostart = false;

type Listener = (payload: unknown) => void;
const listeners = new Map<string, Set<Listener>>();

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((l) => l(payload));
}

export function mockListen<T>(event: string, handler: (payload: T) => void): () => void {
  const set = listeners.get(event) ?? new Set();
  set.add(handler as Listener);
  listeners.set(event, set);
  return () => set.delete(handler as Listener);
}

function update(id: string, patch: Partial<Game>): Game {
  games = games.map((g) => (g.id === id ? { ...g, ...patch } : g));
  return games.find((g) => g.id === id)!;
}

function fakeLaunch(id: string) {
  session = { gameId: id, phase: "launching", startedAt: now(), seconds: 0 };
  emit("game://state", session);
  setTimeout(() => {
    if (session?.gameId !== id) return;
    session = { ...session, phase: "running" };
    emit("game://state", session);
  }, 2500);
  setTimeout(() => {
    if (session?.gameId !== id) return;
    const played = 42 * 60;
    const game = games.find((g) => g.id === id)!;
    emit("library://game", update(id, { playtimeSecs: game.playtimeSecs + played, lastPlayed: now() }));
    emit("game://state", { ...session, phase: "exited", seconds: played });
    session = null;
  }, 9000);
}

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

export async function mockInvoke<T>(cmd: string, args: Record<string, any> = {}): Promise<T> {
  const result = await (async (): Promise<unknown> => {
    switch (cmd) {
      case "get_library":
        return games;
      case "rescan_library": {
        await wait(900);
        const update = { games, errors: [] };
        emit("library://updated", update);
        return update;
      }
      case "launch_game":
        return fakeLaunch(args.id);
      case "set_favorite":
        return update(args.id, { favorite: args.value });
      case "set_hidden":
        return update(args.id, { hidden: args.value });
      case "get_settings":
        return settings;
      case "save_settings":
        return (settings = args.settings);
      case "get_manual_games":
        return manual;
      case "add_manual_game": {
        const m = { id: String(now()), name: args.name, path: args.path, args: args.args };
        manual = [...manual, m];
        return m;
      }
      case "remove_manual_game":
        manual = manual.filter((m) => m.id !== args.id);
        return;
      case "get_session":
        return session;
      case "close_game":
        if (session) {
          emit("game://state", { ...session, phase: "exited", seconds: 60 });
          session = null;
        }
        return true;
      case "resume_game":
        return false;
      case "get_volume":
        return volume;
      case "set_volume":
        volume = { ...volume, level: args.level };
        return;
      case "set_muted":
        volume = { ...volume, muted: args.muted };
        return;
      case "get_autostart":
        return autostart;
      case "set_autostart":
        autostart = args.enabled;
        return;
      case "native_gamepad":
        return false;
      case "list_storefronts":
        return (["steam", "xbox", "epic", "gog", "ubisoft", "ea"] as Store[]).map((store, i) => ({
          store,
          opensIn: i < 3 ? "client" : "browser",
          clientInstalled: i < 4,
        }));
      case "get_catalog": {
        await wait(700);
        const owned: [string, number][] = [
          ["Portal 2", 620],
          ["Celeste", 504230],
          ["Disco Elysium", 632470],
          ["Sekiro: Shadows Die Twice", 814380],
          ["Outer Wilds", 753640],
        ];
        const pass: [string, number][] = [
          ["Hi-Fi Rush", 1817230],
          ["Sea of Thieves", 1172620],
          ["Grounded", 962130],
          ["Pentiment", 1205520],
          ["Microsoft Flight Simulator", 1250410],
          ["Age of Empires IV", 1466860],
        ];
        const items = [
          ...owned.map(([name, appid], i) => ({
            id: `steam:${appid}`,
            source: "steam",
            store: "steam",
            name,
            cover: `${CDN}/${appid}/library_600x900_2x.jpg`,
            hero: `${CDN}/${appid}/library_hero.jpg`,
            playtimeSecs: i * 3600,
          })),
          ...pass.map(([name, appid]) => ({
            id: `gamepass:mock-${appid}`,
            source: "gamepass",
            store: "xbox",
            name,
            cover: `${CDN}/${appid}/library_600x900_2x.jpg`,
            hero: `${CDN}/${appid}/library_hero.jpg`,
            playtimeSecs: 0,
          })),
        ].sort((a, b) => a.name.localeCompare(b.name));
        return { items, steam: settings.steamApiKey ? "ok" : "needsKey", gamepassOk: true };
      }
      case "install_catalog_item":
        console.info("[mock] install", args.id);
        return;
      case "uninstall_game": {
        const game = games.find((g) => g.id === args.id);
        games = games.filter((g) => g.id !== args.id);
        emit("library://updated", { games, errors: [] });
        return game?.store === "steam" ? "started" : "removed";
      }
      case "open_store":
        console.info("[mock] open_store", args);
        return ["steam", "xbox", "epic"].includes(args.store) ? "client" : "browser";
      default:
        // power, exit_to_desktop, toggle_fullscreen, quit_app, refresh_artwork
        console.info(`[mock] ${cmd}`, args);
        return;
    }
  })();
  return result as T;
}
