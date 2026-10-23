// In-browser stand-in for the Rust backend, so the UI can be developed and
// tested with `npm run dev` in any browser.

import type { AppVolume, Capture, Cleanup, Collection, Game, GameMeta, LegendaryJob, ManualGame, NowPlaying, PadKind, PowerSchedule, Profile, SessionState, Settings, Store, SystemInfo, Volume } from "./types";

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
  installDir: `${i % 3 === 0 ? "C:\\Program Files" : "D:\\Giochi"}\\${name.replace(/[:']/g, "")}`,
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
  sizeBytes: i === 9 ? null : (((i * 7919) % 90) + 4) * 1024 ** 3 + i * 123_456_789,
  sizeCheckedAt: store === "steam" || store === "epic" ? null : now() - i * day,
  sizePartial: store === "xbox" && i === 7,
  meta: mockMeta(i, appid),
  metaCheckedAt: now(),
}));

/** A few cards, in various states of completeness. */
function mockMeta(i: number, appid: number): GameMeta | null {
  if (i % 4 === 3) return null;
  const shots = appid ? [`${CDN}/${appid}/header.jpg`, `${CDN}/${appid}/capsule_616x353.jpg`, `${CDN}/${appid}/library_hero.jpg`] : [];
  const genres = [
    ["Azione", "GdR"],
    ["Avventura", "GdR", "Strategia"],
    ["Azione", "Roguelike"],
    ["Platform", "Metroidvania"],
    ["Simulazione", "Indie"],
    ["Azione", "Avventura", "Open world"],
  ][i % 6];
  return {
    summary:
      i % 2 === 0
        ? "Alzati, Senzaluce, e lascia che la grazia ti guidi verso l'Anello Ancestrale. Un vasto mondo pieno di pericoli e meraviglie, dove ogni scelta conta e ogni nemico racconta una storia. Esplora le Terre Interdette, affronta boss leggendari e scopri i segreti di un regno in rovina."
        : "Un'avventura breve ma intensa, da scoprire con calma.",
    genres,
    developer: ["FromSoftware", "Larian Studios", "Supergiant Games", "Team Cherry", "ConcernedApe", "Rockstar Games"][i % 6],
    releaseDate: i % 2 === 0 ? `${12 + (i % 9)} mar ${2016 + (i % 8)}` : `${2016 + (i % 8)}-0${1 + (i % 9)}-1${i % 9}`,
    criticScore: i % 3 === 2 ? null : 70 + ((i * 7) % 30),
    scoreSource: i % 3 === 2 ? null : i % 2 === 0 ? "metacritic" : "igdb",
    screenshots: shots,
    timeToBeat: i % 3 === 1 ? null : { main: (10 + i * 4) * 3600, extra: (25 + i * 8) * 3600, complete: i % 2 ? null : (60 + i * 10) * 3600 },
    igdbChecked: i % 2 === 1,
  };
}

/** Installed applications, for the Apps screen. */
const APPS: [string, number | null][] = [
  ["Netflix", now() - day],
  ["Spotify", now() - 3 * 3600],
  ["Disney+", null],
  ["VLC media player", now() - 9 * day],
  ["Microsoft Edge", null],
  ["Calcolatrice", null],
  ["Foto", null],
  ["Prime Video", null],
];
games = [
  ...games,
  ...APPS.map(([name, lastPlayed], i): Game => ({
    id: `app:mock-app-${i}`,
    store: "app",
    name,
    installDir: null,
    steamAppid: null,
    artwork: {},
    artworkCheckedAt: null,
    favorite: i === 1,
    hidden: false,
    addedAt: now() - 30 * day,
    lastPlayed,
    playtimeSecs: lastPlayed ? 2 * 3600 : 0,
    sizeBytes: null,
    sizeCheckedAt: null,
    sizePartial: false,
  })),
];

let settings: Settings = { steamgriddbKey: null, disabledStores: [], startFullscreen: true, preloadSteam: true, steamApiKey: null, uiSounds: true, tvMode: null, tvAudio: null, performanceMode: false, closeApps: [], overlayEnabled: true, backupDir: null, keyboardAssist: true, steamBigPicture: true, steamGuide: false, wandBacktick: false, wandCombo: false, wandPause: true, haptics: 1, musicControls: true, osdVolume: true, idleScreensaverMin: 10, idleDisplayOffMin: 30, idleSleepMin: 60, idleAction: "sleep", idleWaitDownloads: true, keyboardAuto: true, backupOnExit: true, igdbClientId: null, igdbClientSecret: null, captureCombo: true, companionEnabled: false, hidPads: true, epicLegendary: false };
let manual: ManualGame[] = [];
let session: SessionState | null = null;
let suspended: SessionState[] = [];
let volume: Volume = { level: 0.6, muted: false };
let autostart = false;
let retroarch: string | null = null;
let audioDefault = "speakers";
let radios = { wifi: true, bluetooth: true };
let wifiConnected = "Casa";
let btPaired = ["pad"];
let consoleMode = false;
// `clear_crashes` was called: `startup_info` stops reporting the simulated crash.
let crashesCleared = false;
let backups: { id: string; created: number; automatic: boolean; bytes: number }[] = [
  { id: "b2", created: now() - 2 * day, automatic: false, bytes: 2_400_000 },
];
let romDirs: Record<string, string | null> = {};
let nowPlaying: NowPlaying | null = {
  title: "Blinding Lights",
  artist: "The Weeknd",
  album: "After Hours",
  playing: true,
  source: "Spotify",
  sourceExe: "spotify",
  thumbnail: null,
};
let appVolumes: AppVolume[] = [
  { key: "game", label: "Gioco", level: 1, muted: false, kind: "game", active: true },
  { key: "spotify.exe", label: "Musica · Spotify", level: 0.4, muted: false, kind: "music", active: true },
  { key: "discord.exe", label: "Discord", level: 0.8, muted: false, kind: "chat", active: false },
];

let cleanup: Cleanup = { artworkItems: 3, artworkBytes: 14_200_000, backupItems: 5, backupBytes: 48_000_000, applied: false };

/** legendary in the browser: a pretend account and download queue. */
const legendary = { downloaded: false, account: null as string | null, jobs: [] as LegendaryJob[], nextId: 1 };

function legendaryJob(appName: string, title: string, kind: LegendaryJob["kind"]) {
  const job: LegendaryJob = { id: legendary.nextId++, appName, title, kind, state: "running", phase: "downloading", progress: 0, done: 0, total: 8 * 1024 ** 3, etaSecs: 120, error: null };
  legendary.jobs.push(job);
  const tick = setInterval(() => {
    if (job.state !== "running") return clearInterval(tick);
    job.progress = Math.min(1, (job.progress ?? 0) + 0.05);
    job.done = Math.round(job.total * job.progress);
    job.etaSecs = Math.round((1 - job.progress) * 120);
    if (job.progress >= 1) {
      job.state = "done";
      clearInterval(tick);
    }
    emit("legendary://jobs", legendary.jobs);
  }, 600);
  emit("legendary://jobs", legendary.jobs);
}

function mockStorage() {
  const bytesOn = (prefix: string) => games.filter((g) => g.installDir?.startsWith(prefix)).map((g) => g.sizeBytes ?? 0);
  const onC = bytesOn("C:");
  const onD = bytesOn("D:");
  return {
    drives: [
      { mount: "C:\\", label: "Windows", total: 931 * 1024 ** 3, available: 112 * 1024 ** 3, removable: false, gamesBytes: onC.reduce((a, b) => a + b, 0), games: onC.length },
      { mount: "D:\\", label: "Giochi", total: 1863 * 1024 ** 3, available: 640 * 1024 ** 3, removable: false, gamesBytes: onD.reduce((a, b) => a + b, 0), games: onD.length },
      { mount: "E:\\", label: "", total: 476 * 1024 ** 3, available: 20 * 1024 ** 3, removable: true, gamesBytes: 0, games: 0 },
    ],
    measuring: false,
    unknown: games.filter((g) => g.installDir && g.sizeBytes == null && g.sizeCheckedAt == null).length,
    unreadable: games.filter((g) => g.installDir && g.sizeBytes == null && g.sizeCheckedAt != null).length,
    xboxMount: "C:\\",
  };
}
// Screenshots: the game's hero image stands in for the PNG on disk (artUrl
// passes https through), with a query string telling the shots apart.
const captureUrl = (gameId: string, n: number) =>
  `${CDN}/${games.find((g) => g.id === gameId)?.steamAppid ?? 0}/library_hero.jpg?c=${n}`;
let captures: Capture[] = [0, 3, 6, 9, 0, 12]
  .map((i) => games[i])
  .filter((g) => g.steamAppid)
  .map((g, n) => ({ gameId: g.id, gameName: g.name, path: captureUrl(g.id, n), takenAt: now() - n * day - n * 3600 }));

function mockEmulators() {
  const systems: [string, string, string][] = [
    ["nes", "NES", "fceumm"],
    ["snes", "Super Nintendo", "snes9x"],
    ["n64", "Nintendo 64", "mupen64plus_next"],
    ["gba", "Game Boy Advance", "mgba"],
    ["psx", "PlayStation", "swanstation"],
  ];
  return {
    retroarch: retroarch ?? "C:\\RetroArch-Win64\\retroarch.exe",
    detected: retroarch === null,
    systems: systems.map(([key, name, core]) => ({
      key,
      name,
      core,
      path: romDirs[key] ?? null,
      coreInstalled: key !== "n64",
      roms: romDirs[key] ? 12 : 0,
    })),
  };
}
let profiles: Profile[] = [
  { id: "p1", name: "Giocatore 1", color: "#5ef2c8" },
  { id: "p2", name: "Luca", color: "#4aa3ff" },
];
let activeProfile = "p1";
let collections: Collection[] = [{ id: "c1", name: "Da finire", gameIds: ["steam:mock-0", "gog:mock-12"] }];
let systemInfo: SystemInfo = {
  network: { kind: "wifi", bars: 3, ssid: "Casa", internet: true },
  battery: { percent: 72, charging: false, plugged: false, saver: false },
  bluetooth: { on: true, connected: 1 },
};
let systemDemo = false;
let powerSchedule: PowerSchedule | null = null;
let powerTimer: ReturnType<typeof setInterval> | undefined;

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
  const paused = suspended.find((s) => s.gameId === id);
  if (paused) return fakeResume(paused);
  const name = games.find((g) => g.id === id)?.name ?? id;
  session = { gameId: id, gameName: name, phase: "launching", startedAt: now(), seconds: 0, antiCheat: id.endsWith("-14") };
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

function fakeSuspend() {
  if (!session || session.phase !== "running") throw "Nessun gioco in corso";
  if (suspended.length >= 3) throw "Puoi tenere in pausa al massimo 3 giochi: chiudine uno";
  freeze();
}

/** Freezes the running game, no questions asked (a swap keeps the list's size). */
function freeze() {
  if (!session) return;
  const paused: SessionState = { ...session, phase: "suspended", suspendedAt: now() };
  suspended = [...suspended, paused];
  session = null;
  emit("game://state", paused);
}

/** A battery running low, then the Wi-Fi dropping and coming back: exercises the top bar and its toasts. */
function fakeSystemChanges() {
  if (systemDemo) return;
  systemDemo = true;
  const push = (patch: Partial<SystemInfo>) => {
    systemInfo = { ...systemInfo, ...patch };
    emit("system://status", systemInfo);
  };
  setTimeout(() => push({ battery: { percent: 9, charging: false, plugged: false, saver: true } }), 20_000);
  setTimeout(() => push({ network: { kind: "none", bars: null, ssid: null, internet: false }, bluetooth: { on: true, connected: 2 } }), 40_000);
  setTimeout(() => push({ network: { kind: "ethernet", bars: null, ssid: null, internet: true }, battery: { percent: 12, charging: true, plugged: true, saver: false } }), 55_000);
}

function fakeResume(paused: SessionState) {
  if (session) freeze();
  suspended = suspended.filter((s) => s.gameId !== paused.gameId);
  session = { ...paused, phase: "running", suspendedAt: null };
  emit("game://state", session);
}

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

let cheatScan: { kind: string; count: number; hits: { address: number; value: string; frozen: boolean }[] } | null = null;
let cheatFrozen: { address: number; value: string; frozen: boolean }[] = [];
/** "Shut down in…": counts the last minute down out loud, then just logs. */
function fakeSchedule(action: PowerSchedule["action"], mode: { kind: "in"; secs: number } | { kind: "atDownloadsEnd" }) {
  clearInterval(powerTimer);
  // Like the backend, "at the end of the downloads" has no time until the
  // downloads are over; nothing downloads here, so that's a quiet minute away.
  const armAt = mode.kind === "atDownloadsEnd" ? now() + 60 : 0;
  powerSchedule = { action, at: mode.kind === "in" ? now() + mode.secs : null, atDownloadsEnd: mode.kind === "atDownloadsEnd" };
  emit("power://schedule", powerSchedule);
  powerTimer = setInterval(() => {
    if (!powerSchedule) return;
    if (powerSchedule.at === null) {
      if (now() < armAt) return;
      powerSchedule = { ...powerSchedule, at: now() + 60 };
      emit("power://schedule", powerSchedule);
    }
    const left = Math.max(0, (powerSchedule.at ?? 0) - now());
    if (left <= 60) emit("power://countdown", { action, secondsLeft: left });
    if (left === 0) {
      clearInterval(powerTimer);
      powerSchedule = null;
      emit("power://schedule", null);
      console.info("[mock] power", action);
    }
  }, 1000);
  return powerSchedule;
}

function mockPadKind(): PadKind {
  try {
    const kind = localStorage.getItem("gm-mock-pad");
    if (kind === "playstation" || kind === "nintendo") return kind;
  } catch {
    // ignore
  }
  return "xbox";
}

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
      case "get_suspended":
        return suspended;
      case "suspend_game":
        return fakeSuspend();
      case "resume_suspended":
        return fakeResume(suspended.find((s) => s.gameId === args.gameId) ?? (() => { throw "Questo gioco non è in pausa"; })());
      case "close_suspended": {
        const paused = suspended.find((s) => s.gameId === args.gameId);
        if (!paused) throw "Questo gioco non è in pausa";
        suspended = suspended.filter((s) => s.gameId !== args.gameId);
        emit("game://state", { ...paused, phase: "exited", seconds: 15 * 60 });
        return;
      }
      case "close_game":
        if (session) {
          emit("game://state", { ...session, phase: "exited", seconds: 60 });
          session = null;
        }
        return true;
      case "resume_game":
        return false;
      case "wand_installed":
        return true;
      case "open_wand":
      case "cheats_status":
        return { available: true, reason: null, scan: cheatScan, frozen: cheatFrozen };
      case "cheats_search": {
        await wait(400);
        const count = args.fresh || !cheatScan ? 48213 : Math.max(1, Math.floor(cheatScan.count / 40));
        const hits =
          count <= 20
            ? Array.from({ length: count }, (_, i) => ({ address: 0x7ff6a1b20000 + i * 0x40, value: args.value, frozen: false }))
            : [];
        return (cheatScan = { kind: args.kind, count, hits });
      }
      case "cheats_set":
      case "cheats_reset":
        return;
      case "cheats_freeze": {
        const hit = cheatScan?.hits.find((h) => h.address === args.address);
        if (hit) cheatFrozen = [...cheatFrozen.filter((f) => f.address !== hit.address), { ...hit, value: args.value ?? hit.value, frozen: true }];
        return;
      }
      case "cheats_unfreeze":
        cheatFrozen = cheatFrozen.filter((f) => f.address !== args.address);
        return;
      case "open_wand_app":
        return;
      case "get_game_updates":
        return games[1] ? [{ gameId: games[1].id, state: "downloading", done: 1.2 * 1024 ** 3, total: 4.5 * 1024 ** 3, progress: 0.27 }] : [];
      case "get_volume":
        return volume;
      case "set_volume":
        volume = { ...volume, level: args.level };
        emit("audio://volume", { ...volume, own: true });
        return;
      case "set_muted":
        volume = { ...volume, muted: args.muted };
        emit("audio://volume", { ...volume, own: true });
        return;
      case "get_autostart":
        return autostart;
      case "set_autostart":
        autostart = args.enabled;
        return;
      case "native_gamepad":
        return false;
      case "rumble":
        return;
      case "get_profiles":
        return { profiles, active: activeProfile };
      case "add_profile":
        profiles = [...profiles, { id: `p${now()}`, name: args.name, color: "#f5b942" }];
        return { profiles, active: activeProfile };
      case "rename_profile":
        profiles = profiles.map((p) => (p.id === args.id ? { ...p, name: args.name } : p));
        return { profiles, active: activeProfile };
      case "remove_profile":
        if (profiles.length <= 1) throw "Non puoi eliminare l'unico profilo";
        profiles = profiles.filter((p) => p.id !== args.id);
        if (activeProfile === args.id) activeProfile = profiles[0].id;
        return { profiles, active: activeProfile };
      case "switch_profile":
        activeProfile = args.id;
        emit("library://updated", { games, errors: [] });
        return { profiles, active: activeProfile };
      case "get_collections":
        return collections;
      case "steam_extras": {
        await wait(400);
        const names = ["Primo passo", "Esploratore", "Senza paura", "Collezionista", "Leggenda", "Perfezionista"];
        return {
          news: [
            { title: "Aggiornamento 1.12: nuove sfide stagionali", url: "https://store.steampowered.com", date: now() - 2 * day, snippet: "Arrivano tre nuove sfide, bilanciamento delle armi e correzioni alla modalità cooperativa." },
            { title: "Note sulla patch di ottobre", url: "https://store.steampowered.com", date: now() - 9 * day, snippet: "Migliorate le prestazioni su schede video di fascia media e ridotti i tempi di caricamento." },
          ],
          achievements: names.map((name, i) => ({
            name,
            description: "Descrizione dell'obiettivo",
            icon: null,
            achieved: i < 4,
            unlockTime: i < 4 ? now() - i * day : 0,
          })),
          achievementStatus: settings.steamApiKey ? "ok" : "needsKey",
        };
      }
      case "wishlist_deals":
        await wait(500);
        return {
          status: "ok",
          items: [
            [1593500, "God of War", 60, "49,99€", "19,99€"],
            [1716740, "Starfield", 35, "69,99€", "45,49€"],
            [1817070, "Marvel's Spider-Man Remastered", 50, "59,99€", "29,99€"],
          ].map(([appid, name, discount, initial, price]) => ({
            appid,
            name,
            discount,
            initial,
            price,
            cover: `${CDN}/${appid}/library_600x900_2x.jpg`,
            hero: `${CDN}/${appid}/library_hero.jpg`,
          })),
        };
      case "get_sessions": {
        // Three weeks of plausible evenings.
        const out = [];
        const today = new Date();
        today.setHours(21, 0, 0, 0);
        for (let d = 0; d < 21; d++) {
          if ((d * 7) % 5 === 3) continue;
          const g = games[(d * 3) % games.length];
          const seconds = 1800 + ((d * 977) % 7200);
          const end = Math.floor(today.getTime() / 1000) - d * day;
          out.push({ gameId: g.id, profileId: activeProfile, start: end - seconds, end, seconds });
        }
        return out;
      }
      case "add_collection": {
        const c = { id: `c${now()}`, name: args.name, gameIds: [] };
        collections = [...collections, c];
        return c;
      }
      case "remove_collection":
        collections = collections.filter((c) => c.id !== args.id);
        return;
      case "set_in_collection":
        collections = collections.map((c) =>
          c.id !== args.collectionId
            ? c
            : { ...c, gameIds: args.value ? [...c.gameIds.filter((g) => g !== args.gameId), args.gameId] : c.gameIds.filter((g) => g !== args.gameId) },
        );
        return collections;
      case "get_controllers":
        // `localStorage.setItem("gm-mock-pad", "playstation")` to preview other glyph sets.
        return [{ slot: 0, wired: false, level: 2, kind: mockPadKind(), name: "Controller 1" }];
      case "get_system_status":
        fakeSystemChanges();
        return systemInfo;
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
            sizeBytes: (appid % 7) * 11 * 1024 ** 3 + 20 * 1024 ** 3,
          })),
        ].sort((a, b) => a.name.localeCompare(b.name));
        return { items, steam: settings.steamApiKey ? "ok" : "needsKey", gamepassOk: true };
      }
      case "get_cloud_catalog": {
        await wait(1200);
        const cloud: [string, number][] = [
          ["Forza Horizon 5", 1551360],
          ["Hellblade: Senua's Sacrifice", 414340],
          ["Halo Infinite", 1240440],
        ];
        return cloud.map(([name, appid]) => ({
          id: `cloud:mock${appid}:${name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`,
          source: "cloud",
          store: "xbox",
          name,
          cover: `${CDN}/${appid}/library_600x900_2x.jpg`,
          hero: `${CDN}/${appid}/library_hero.jpg`,
          playtimeSecs: 0,
        }));
      }
      case "display_modes":
        return {
          modes: [
            { width: 3840, height: 2160, refresh: 60 },
            { width: 2560, height: 1440, refresh: 144 },
            { width: 1920, height: 1080, refresh: 120 },
            { width: 1920, height: 1080, refresh: 60 },
          ],
          current: { width: 2560, height: 1440, refresh: 144 },
        };
      case "audio_outputs":
        return [
          ["speakers", "Altoparlanti"],
          ["tv", "TV (HDMI)"],
          ["headset", "Cuffie Xbox Wireless"],
        ].map(([id, name]) => ({ id, name, default: id === audioDefault }));
      case "set_audio_output":
        audioDefault = args.id;
        return;
      case "get_radios":
        return radios;
      case "set_radio":
        radios = { ...radios, [args.bluetooth ? "bluetooth" : "wifi"]: args.on };
        return;
      case "wifi_networks":
        await wait(900);
        return [
          ["Casa", 4, true],
          ["Casa-5G", 3, true],
          ["Ospiti", 2, false],
          ["Vicino", 1, true],
        ].map(([ssid, bars, secured]) => ({ ssid, bars, secured, connected: ssid === wifiConnected }));
      case "wifi_connect":
        await wait(1200);
        if (args.ssid !== "Ospiti" && (!args.password || args.password.length < 8)) throw "password sbagliata";
        wifiConnected = args.ssid;
        return;
      case "bluetooth_devices":
        await wait(args.discover ? 2500 : 300);
        return [
          ["pad", "Xbox Wireless Controller"],
          ["buds", "Cuffie Bluetooth"],
          ["pad2", "DualSense Wireless Controller"],
        ]
          .filter(([id]) => args.discover || btPaired.includes(id))
          .map(([id, name]) => ({ id, name, paired: btPaired.includes(id), connected: id === "pad" }));
      case "bluetooth_pair":
        await wait(1500);
        btPaired = [...btPaired, args.id];
        return;
      case "bluetooth_unpair":
        btPaired = btPaired.filter((p) => p !== args.id);
        return;
      case "save_info":
        await wait(600);
        return {
          known: true,
          locations: ["C:\\Users\\Max\\Documents\\Saved Games\\Demo"],
          backups,
          backupDir: "C:\\Users\\Max\\AppData\\Roaming\\GameMode\\backups",
        };
      case "backup_game": {
        await wait(800);
        const b = { id: `b${now()}`, created: now(), automatic: false, bytes: 2_500_000 };
        backups = [b, ...backups];
        return b;
      }
      case "restore_backup":
        await wait(800);
        backups = [{ id: `a${now()}`, created: now(), automatic: true, bytes: 2_500_000 }, ...backups];
        return;
      case "delete_backup":
        backups = backups.filter((b) => b.id !== args.backupId);
        return;
      case "get_console_mode":
        return { enabled: consoleMode, active: false, afterUpdate: false, desktopOnce: false };
      case "set_console_mode":
        consoleMode = args.enabled;
        return { enabled: consoleMode, active: false, afterUpdate: false, desktopOnce: false };
      case "startup_info": {
        // `?safe` or `?recovered` in the browser's URL simulate the watchdog.
        const q = new URLSearchParams(window.location.search);
        const crashed = !crashesCleared && (q.has("safe") || q.has("recovered"));
        return {
          recovered: q.has("recovered") ? 1 : 0,
          safeMode: q.has("safe"),
          lastCrash: crashed ? { at: now() - 40, exitCode: 0xc0000409, hung: false } : null,
          lastCrashDescription: crashed ? "Errore interno (panic)" : null,
          lastPanic: crashed && q.has("safe") ? "panic at src/launcher.rs:120: called `Option::unwrap()` on a `None` value" : null,
          crashCount: !crashed ? 0 : q.has("safe") ? 4 : 1,
          logPath: "C:\\Users\\demo\\AppData\\Local\\it.maxpolce.gamemode\\logs\\gamemode.log",
        };
      }
      case "read_log":
        return [
          "2026-10-23 08:00:01 [app] INFO  gamemode_lib: GameMode 0.2.6 starting, args []",
          "2026-10-23 08:00:01 [app] INFO  gamemode_lib: console mode on, recovered 0, safe mode false, after update false",
          "2026-10-23 08:12:40 [app] INFO  gamemode_lib: launching Elden Ring (steam:mock-0)",
          "2026-10-23 09:02:11 [app] WARN  launcher: saving session failed: permesso negato",
          "2026-10-23 09:02:12 [app] PANIC src/launcher.rs:120: called `Option::unwrap()` on a `None` value",
          "2026-10-23 09:02:13 [watchdog] INFO  shell: GameMode ended with exit code 0xC0000409",
          "2026-10-23 09:02:15 [watchdog] INFO  shell: GameMode started again with [\"--recovered\", \"1\"]",
        ].join("\n");
      case "export_log":
        await wait(500);
        return "/Users/demo/Desktop/gamemode-log.txt";
      case "clear_crashes":
        crashesCleared = true;
        return;
      case "check_update":
        await wait(700);
        return { version: "0.2.1", notes: "Correzioni e miglioramenti" };
      case "system_updates":
        if (args.force) await wait(2500);
        return {
          state: "ready",
          items: [
            { title: "Aggiornamento cumulativo 2026-10 per Windows 11 Version 25H2 (KB5071234)", bytes: 812 * 1024 ** 2, downloaded: true },
            { title: "Aggiornamento per Microsoft Defender Antivirus (KB2267602)", bytes: 2.4 * 1024 ** 2, downloaded: true },
          ],
          checkedAt: now() - 1800,
          error: null,
        };
      case "install_update":
        await wait(1500);
        throw "Aggiornamento simulato: nel browser non si installa nulla";
      case "backup_all":
        await wait(1500);
        return { saved: 9, skipped: 7 };
      case "emulator_status":
        return mockEmulators();
      case "set_retroarch_path":
        retroarch = args.path;
        return mockEmulators();
      case "set_rom_dir":
        romDirs = { ...romDirs, [args.system]: args.path };
        return mockEmulators();
      case "install_catalog_item":
        console.info("[mock] install", args.id);
        return;
      case "uninstall_game": {
        const game = games.find((g) => g.id === args.id);
        games = games.filter((g) => g.id !== args.id);
        emit("library://updated", { games, errors: [] });
        return game?.store === "steam" ? "started" : "removed";
      }
      case "schedule_power":
        return fakeSchedule(args.action, args.mode);
      case "cancel_power":
        clearInterval(powerTimer);
        if (powerSchedule) emit("power://schedule", (powerSchedule = null));
        return;
      case "get_power_schedule":
        return powerSchedule;
      case "rest_mode":
        emit("idle://state", { stage: "displayOff" });
        return;
      case "idle_wake":
        emit("idle://state", { stage: "active" });
        return;
      case "touch_input":
        return;
      case "take_screenshot": {
        if (!session || session.phase !== "running") throw "Nessun gioco in corso";
        await wait(300);
        const c: Capture = { gameId: session.gameId, gameName: session.gameName, path: captureUrl(session.gameId, Date.now()), takenAt: now() };
        captures = [c, ...captures];
        emit("capture://saved", c);
        return c;
      }
      case "list_captures":
        return captures.filter((c) => !args.gameId || c.gameId === args.gameId);
      case "delete_capture":
        captures = captures.filter((c) => c.path !== args.path);
        return;
      case "capture_notice":
        return null;
      case "open_store":
        console.info("[mock] open_store", args);
        return ["steam", "xbox", "epic"].includes(args.store) ? "client" : "browser";
      case "media_now_playing":
        return nowPlaying;
      case "media_control": {
        if (!nowPlaying) throw "Nessuna musica in riproduzione";
        const tracks = ["Blinding Lights", "Save Your Tears", "Starboy"];
        const i = tracks.indexOf(nowPlaying.title);
        const playing = args.action === "play" ? true : args.action === "pause" ? false : args.action === "toggle" ? !nowPlaying.playing : nowPlaying.playing;
        const title = args.action === "next" ? tracks[(i + 1) % tracks.length] : args.action === "prev" ? tracks[(i + tracks.length - 1) % tracks.length] : nowPlaying.title;
        nowPlaying = { ...nowPlaying, playing, title };
        emit("media://now-playing", nowPlaying);
        return;
      }
      case "open_music":
        nowPlaying ??= { title: "Blinding Lights", artist: "The Weeknd", album: "After Hours", playing: true, source: "Spotify", sourceExe: "spotify", thumbnail: null };
        emit("media://now-playing", nowPlaying);
        return;
      case "app_volumes":
        return session ? appVolumes : appVolumes.filter((v) => v.kind !== "game");
      case "set_app_volume":
        appVolumes = appVolumes.map((v) => (v.key === args.key ? { ...v, level: args.level } : v));
        return;
      case "set_app_muted":
        appVolumes = appVolumes.map((v) => (v.key === args.key ? { ...v, muted: args.muted } : v));
        return;
      case "type_text":
      case "type_key":
        console.info(`[mock] ${cmd}`, args);
        return;
      case "overlay_keyboard":
      case "keyboard_hide":
        return;
      case "storage_overview":
        await wait(300);
        return mockStorage();
      case "storage_cleanup": {
        await wait(args.apply ? 900 : 300);
        const out = { ...cleanup, applied: !!args.apply };
        if (args.apply) cleanup = { artworkItems: 0, artworkBytes: 0, backupItems: 0, backupBytes: 0, applied: false };
        return out;
      }
      case "storage_remeasure":
        setTimeout(() => {
          games = games.map((g) => (g.sizeBytes == null && g.installDir ? { ...g, sizeBytes: 12 * 1024 ** 3, sizeCheckedAt: now() } : g));
          games.forEach((g) => emit("library://game", g));
          emit("storage://measured", null);
        }, 1500);
        return;
      case "power_options":
        return { sleep: true, hibernate: false };
      case "power":
        console.info("[mock] power", args);
        // A pretend nap, to see the wake-up flow in the browser.
        if (args.action === "sleep") {
          emit("power://state", { kind: "suspended" });
          setTimeout(() => emit("power://state", { kind: "resumed", slept: 3 }), 3000);
        }
        return;
      case "metadata_running":
        return false;
      case "companion_status":
        return {
          enabled: settings.companionEnabled,
          running: settings.companionEnabled,
          address: settings.companionEnabled ? "http://192.168.1.20:7373" : null,
          url: settings.companionEnabled ? "http://192.168.1.20:7373/?t=demo" : null,
          qrSvg: settings.companionEnabled
            ? '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8" shape-rendering="crispEdges"><rect width="8" height="8" fill="#fff"/><path fill="#07090d" d="M1 1h3v3H1zM5 1h2v2H5zM1 5h2v2H1zM4 4h1v1H4zM5 5h2v2H5z"/></svg>'
            : null,
          pin: settings.companionEnabled ? "482913" : null,
          firewall: "ok",
          error: null,
        };
      case "companion_firewall":
        await wait(500);
        return true;
      case "legendary_status":
        return {
          supported: true,
          downloaded: legendary.downloaded,
          enabled: settings.epicLegendary,
          account: legendary.account,
          eglSignedIn: true,
          eglGames: legendary.account ? 2 : 0,
          version: "0.21.1",
          projectUrl: "https://github.com/legendary-gl/legendary",
          licenseUrl: "https://github.com/legendary-gl/legendary/blob/master/LICENSE",
        };
      case "legendary_enable":
        if (args.on && !legendary.downloaded) {
          for (const p of [0.2, 0.5, 0.8, 1]) {
            await wait(300);
            emit("legendary://download", p);
          }
          legendary.downloaded = true;
        }
        settings = { ...settings, epicLegendary: !!args.on };
        return settings;
      case "legendary_login":
        setTimeout(() => {
          legendary.account = "Giocatore";
          emit("legendary://login", { Ok: legendary.account });
          emit("legendary://changed", null);
        }, 1500);
        return;
      case "legendary_import_auth":
        await wait(800);
        legendary.account = "Giocatore";
        return legendary.account;
      case "legendary_logout":
        legendary.account = null;
        return;
      case "legendary_import_games":
        legendaryJob("", "Giochi dell'Epic Games Launcher", "import");
        return;
      case "legendary_jobs":
        return legendary.jobs;
      case "legendary_install":
        legendaryJob(args.appName, args.appName, "install");
        return;
      case "legendary_game_action": {
        const game = games.find((g) => g.id === args.id);
        legendaryJob(args.id, game?.name ?? args.id, args.action);
        return;
      }
      case "legendary_cancel": {
        const job = legendary.jobs.find((j) => j.id === args.jobId);
        if (job) job.state = "cancelled";
        emit("legendary://jobs", legendary.jobs);
        return;
      }
      case "legendary_install_targets":
        return [
          { path: "C:\\Games", free: 412 * 1024 ** 3, default: true },
          { path: "D:\\Games", free: 1200 * 1024 ** 3, default: false },
        ];
      default:
        // exit_to_desktop, toggle_fullscreen, quit_app, refresh_artwork, refresh_metadata
        console.info(`[mock] ${cmd}`, args);
        return;
    }
  })();
  return result as T;
}
