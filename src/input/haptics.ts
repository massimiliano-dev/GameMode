// Controller vibration for interface feedback. On Windows the backend drives
// the pads through XInput and knows the setting and whether a game is
// running; in a plain browser the Gamepad API's dual-rumble stands in, so
// the feel can be tried during development.

import { api, isTauri } from "../lib/api";

export type Pattern = "tick" | "select" | "error" | "launch" | "notify";

/** 0 off, 1 light, 2 normal — mirrors `Settings.haptics`. */
let level = 1;

export function setHaptics(value: number) {
  level = value;
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
  if (level === 0) return;
  if (isTauri) {
    void api.rumble(pattern).catch(() => {});
    return;
  }
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
