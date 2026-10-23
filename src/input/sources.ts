// Input sources feeding the dispatcher.

import { api, isTauri, onWindow } from "../lib/api";
import type { Action } from "../lib/types";
import { dispatch } from "./dispatcher";
import { setNativeHaptics } from "./haptics";
import { remoteText } from "./remote";

const KEYS: Record<string, Action> = {
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  Enter: "confirm",
  " ": "confirm",
  Escape: "back",
  Backspace: "back",
  q: "lb",
  e: "rb",
  x: "x",
  y: "y",
  m: "menu",
  v: "view",
  "/": "view",
  g: "guide",
  "1": "lt",
  "3": "rt",
};

/** Keys that still navigate while a text field has focus. */
const KEYS_IN_FIELDS = new Set(["ArrowUp", "ArrowDown", "Escape", "Enter"]);

function isTextField(el: Element | null) {
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;
}

function keyboard() {
  window.addEventListener("keydown", (e) => {
    if (e.key === "F11") {
      e.preventDefault();
      void api.toggleFullscreen();
      return;
    }
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const action = KEYS[e.key] ?? KEYS[e.key.toLowerCase()];
    if (!action) return;
    controllerActive();
    if (isTextField(document.activeElement) && !KEYS_IN_FIELDS.has(e.key)) return;
    e.preventDefault();
    dispatch(action, e.repeat);
  });
}

// Standard Gamepad API mapping (Xbox layout).
const BUTTONS: [number, Action][] = [
  [0, "confirm"],
  [1, "back"],
  [2, "x"],
  [3, "y"],
  [4, "lb"],
  [5, "rb"],
  [6, "lt"],
  [7, "rt"],
  [8, "view"],
  [9, "menu"],
  [12, "up"],
  [13, "down"],
  [14, "left"],
  [15, "right"],
  [16, "guide"],
];
const REPEATABLE = new Set<Action>(["up", "down", "left", "right"]);
const REPEAT_DELAY = 400;
const REPEAT_RATE = 80;
const STICK = 0.5;

/** Hides the mouse cursor while a controller is driving the UI. */
function controllerActive() {
  document.body.classList.add("no-cursor");
}

function browserGamepad() {
  const held = new Map<Action, { since: number; last: number }>();

  const frame = (t: number) => {
    requestAnimationFrame(frame);
    if (!document.hasFocus()) {
      held.clear();
      return;
    }
    const pressed = new Set<Action>();
    for (const pad of navigator.getGamepads()) {
      if (!pad) continue;
      for (const [i, action] of BUTTONS) if (pad.buttons[i]?.pressed) pressed.add(action);
      const [x = 0, y = 0] = pad.axes;
      if (x < -STICK) pressed.add("left");
      if (x > STICK) pressed.add("right");
      if (y < -STICK) pressed.add("up");
      if (y > STICK) pressed.add("down");
    }
    for (const action of pressed) {
      const state = held.get(action);
      if (!state) {
        held.set(action, { since: t, last: t });
        controllerActive();
        dispatch(action, false, "pad");
      } else if (REPEATABLE.has(action) && t - state.since >= REPEAT_DELAY && t - state.last >= REPEAT_RATE) {
        state.last = t;
        dispatch(action, true, "pad");
      }
    }
    for (const action of held.keys()) if (!pressed.has(action)) held.delete(action);
  };
  requestAnimationFrame(frame);
}

/** Moving the mouse over a focusable element focuses it, like a cursor. */
function pointer() {
  window.addEventListener("mousemove", () => document.body.classList.remove("no-cursor"));
  window.addEventListener("mouseover", (e) => {
    // Content scrolling under a parked cursor must not steal controller focus.
    if (document.body.classList.contains("no-cursor")) return;
    const el = (e.target as HTMLElement).closest<HTMLElement>("[data-nav]");
    if (el && el !== document.activeElement && !isTextField(document.activeElement)) {
      el.focus({ preventScroll: true });
    }
  });
}

let setup: Promise<boolean> | null = null;

/**
 * Wires every source. Returns whether the native XInput backend is active;
 * when it is, the browser Gamepad API stays off so presses don't double up.
 */
export function setupInput(): Promise<boolean> {
  setup ??= start();
  return setup;
}

async function start(): Promise<boolean> {
  keyboard();
  pointer();
  remoteText();
  const native = isTauri ? await api.nativeGamepad().catch(() => false) : false;
  setNativeHaptics(native);
  // Also without XInput: the phone companion's presses arrive here too.
  if (isTauri) {
    onWindow<{ action: Action; repeat: boolean }>("gamepad://nav", (e) => {
      controllerActive();
      dispatch(e.action, e.repeat, "pad");
    });
  }
  if (!native) browserGamepad();
  return native;
}
