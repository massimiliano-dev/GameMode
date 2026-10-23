// Input sources feeding the dispatcher.

import { api, isTauri, on, onWindow } from "../lib/api";
import type { Action, Controller } from "../lib/types";
import { dispatch } from "./dispatcher";

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

/** True while the native HID backend has a Sony/Nintendo pad open. */
let nativeHidPads = false;

/**
 * Pads the native backend already reads, so the browser must skip them:
 * XInput ones always, Sony/Nintendo ones while the HID backend has one.
 * Chromium's ids look like "Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)".
 */
export function nativelyHandled(id: string, native: boolean, hidPads: boolean) {
  if (!native) return false;
  const lower = id.toLowerCase();
  if (lower.includes("xinput") || lower.includes("vendor: 045e")) return true;
  return hidPads && (lower.includes("vendor: 054c") || lower.includes("vendor: 057e"));
}

function browserGamepad(native: boolean) {
  const held = new Map<Action, { since: number; last: number }>();

  const frame = (t: number) => {
    requestAnimationFrame(frame);
    if (!document.hasFocus()) {
      held.clear();
      return;
    }
    const pressed = new Set<Action>();
    for (const pad of navigator.getGamepads()) {
      if (!pad || nativelyHandled(pad.id, native, nativeHidPads)) continue;
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
 * when it is, the browser Gamepad API only covers pads the backend doesn't
 * read (generic HID pads, while GameMode has focus) so presses don't double up.
 */
export function setupInput(): Promise<boolean> {
  setup ??= start();
  return setup;
}

async function start(): Promise<boolean> {
  keyboard();
  pointer();
  const native = isTauri ? await api.nativeGamepad().catch(() => false) : false;
  if (native) {
    onWindow<{ action: Action; repeat: boolean }>("gamepad://nav", (e) => {
      controllerActive();
      dispatch(e.action, e.repeat, "pad");
    });
    const track = (list: Controller[]) => (nativeHidPads = list.some((c) => c.kind !== "xbox"));
    on<Controller[]>("gamepad://controllers", track);
    void api.getControllers().then(track).catch(() => {});
  }
  browserGamepad(native);
  return native;
}
