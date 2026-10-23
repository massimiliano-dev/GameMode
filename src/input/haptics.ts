// Controller vibration for interface feedback. With the native XInput backend
// the Rust side drives the pads and knows the setting and whether a game is
// running; otherwise (plain browser, or Tauri on a platform without XInput)
// the Gamepad API's dual-rumble stands in, so the feel can be tried during
// development.

import { api, isTauri } from "../lib/api";

export type Pattern = "tick" | "select" | "error" | "launch" | "notify";

/** 0 off, 1 light, 2 normal — mirrors `Settings.haptics`; only the browser fallback reads it. */
let level = 1;

export function setHaptics(value: number) {
  level = value;
}

/** Whether the native backend owns the pads (the result of `setupInput`). */
let native = isTauri;

export function setNativeHaptics(value: boolean) {
  native = value;
}

/** Beats as `[startMs, lengthMs, strong, weak]`, same as the Rust side. */
const BEATS: Record<Pattern, [number, number, number, number][]> = {
  tick: [[0, 30, 0, 0.4]],
  select: [[0, 60, 0.25, 0.65]],
  error: [[0, 140, 0.75, 0.2]],
  launch: [
    [0, 70, 0.55, 0.3],
    [130, 110, 0.9, 0.5],
  ],
  notify: [
    [0, 40, 0, 0.55],
    [100, 40, 0, 0.55],
  ],
};

export function rumble(pattern: Pattern) {
  if (native) {
    // The backend reads the saved level itself, so every window stays in sync.
    void api.rumble(pattern).catch(() => {});
    return;
  }
  if (level === 0) return;
  const factor = level >= 2 ? 1 : 0.45;
  try {
    for (const pad of navigator.getGamepads()) {
      const actuator = pad?.vibrationActuator;
      if (!actuator?.playEffect) continue;
      for (const [start, length, strong, weak] of BEATS[pattern]) {
        // One effect at a time per pad: a second playEffect would preempt the first.
        setTimeout(() => {
          void actuator
            .playEffect("dual-rumble", { duration: length, strongMagnitude: strong * factor, weakMagnitude: weak * factor })
            .catch(() => {});
        }, start);
      }
    }
  } catch {
    // No Gamepad API: stay still.
  }
}
