// Inactivity and scheduled power, as the backend reports them: the
// screensaver stage, the pending "shut down in…" and its final countdown.

import { create } from "zustand";

import { onAnyAction } from "../input/dispatcher";
import { api, isTauri, on } from "../lib/api";
import type { IdleStage, PowerCountdown, PowerSchedule } from "../lib/types";
import { useApp } from "./app";

interface IdleState {
  stage: IdleStage;
  schedule: PowerSchedule | null;
  countdown: PowerCountdown | null;
  init: () => void;
  wake: () => void;
  cancelPower: () => Promise<void>;
}

/** How often at most the backend is told about frontend input. */
const TOUCH_EVERY = 5_000;
/** Without a backend (plain browser) the stages are timed here. */
const FALLBACK_TICK = 15_000;
let initialized = false;
let lastTouch = 0;
let lastInput = Date.now();
/** A stage set by the backend (rest mode in the mock) holds until a wake. */
let pinned = false;

export const useIdle = create<IdleState>((set, get) => ({
  stage: "active",
  schedule: null,
  countdown: null,

  init: () => {
    if (initialized) return;
    initialized = true;
    on<{ stage: IdleStage }>("idle://state", ({ stage }) => {
      pinned = stage !== "active";
      set({ stage });
    });
    on<PowerSchedule | null>("power://schedule", (schedule) => set({ schedule, ...(schedule ? {} : { countdown: null }) }));
    on<PowerCountdown>("power://countdown", (c) => {
      set({ countdown: c.secondsLeft > 0 ? c : null });
      // The warning must be seen: leave the screensaver or the dark screen.
      if (c.secondsLeft > 0 && get().stage !== "active") get().wake();
    });
    void api.getPowerSchedule().then((schedule) => set({ schedule })).catch(() => {});

    // Controller presses through the Gamepad API and mouse moves inside the
    // page: Windows sees the latter itself, the backend hears both anyway.
    const touch = () => {
      lastInput = Date.now();
      if (lastInput - lastTouch < TOUCH_EVERY) return;
      lastTouch = lastInput;
      void api.touchInput().catch(() => {});
    };
    onAnyAction(touch);
    window.addEventListener("mousemove", touch, { passive: true });

    if (!isTauri) {
      setInterval(() => {
        const settings = useApp.getState().settings;
        if (!settings || pinned || get().countdown) return;
        const idle = (Date.now() - lastInput) / 60_000;
        const off = settings.idleDisplayOffMin > 0 && idle >= settings.idleDisplayOffMin;
        const saver = settings.idleScreensaverMin > 0 && idle >= settings.idleScreensaverMin;
        set({ stage: off ? "displayOff" : saver ? "screensaver" : "active" });
      }, FALLBACK_TICK);
    }
  },

  wake: () => {
    lastInput = Date.now();
    pinned = false;
    if (get().stage === "active") return;
    set({ stage: "active" });
    void api.idleWake().catch(() => {});
  },

  cancelPower: async () => {
    set({ schedule: null, countdown: null });
    await api.cancelPower().catch(() => {});
  },
}));
