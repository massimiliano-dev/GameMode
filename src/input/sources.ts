// Input sources feeding the dispatcher.

import { api, isTauri, on, onWindow } from "../lib/api";
import type { Action, Controller } from "../lib/types";
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

/**
 * True while the native backend reports any controller. A supported
 * Sony/Nintendo pad is then read natively: straight over HID, or through
 * the virtual Xbox pad Steam Input or DS4Windows make of it (the HID copy
 * is hidden then, or the HID backend is off).
 */
let nativePads = false;

/** Pads the Rust HID backend reads, as Chromium's "vendor product" ids (see `Model::from_ids`). */
const HID_MODELS = new Set(["054c 0ce6", "054c 0df2", "054c 05c4", "054c 09cc", "057e 2009"]);

/**
 * Pads the native backend already reads, so the browser must skip them:
 * XInput ones always, the supported Sony/Nintendo ones while it reports
 * any controller. Others (Joy-Con, generic HID pads) stay with the browser.
 * Chromium's ids look like "Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)".
 */
export function nativelyHandled(id: string, native: boolean, pads: boolean) {
  if (!native) return false;
  const lower = id.toLowerCase();
  if (lower.includes("xinput") || lower.includes("vendor: 045e")) return true;
  const ids = /vendor: ([0-9a-f]{4}) product: ([0-9a-f]{4})/.exec(lower);
  return pads && !!ids && HID_MODELS.has(`${ids[1]} ${ids[2]}`);
}

/**
 * The same physical pad can reach GameMode twice: through the native backend
 * (XInput, HID) and through the browser's Gamepad API, whose ids don't always
 * give it away (third-party Xbox pads, Steam Input's virtual pad). A press
 * that arrives from the other path within ECHO_MS is that echo and is
 * dropped; a browser pad caught echoing ECHO_STRIKES times is skipped for good.
 */
const ECHO_MS = 120;
const ECHO_STRIKES = 2;
/** Browser repeats pause while the native path is still sending this action. */
const NATIVE_QUIET = REPEAT_DELAY + 200;
/** Browser pads ("index:id") found to mirror a native one. */
const twins = new Set<string>();
const strikes = new Map<string, number>();
/** Last fresh press per action: which browser pad sent it, or null for native. */
const lastPress = new Map<Action, { t: number; from: string | null }>();
/** Last native event (press or repeat) per action. */
const lastNative = new Map<Action, number>();

/** Whether a fresh press is the other path's echo of one just dispatched. */
export function isEcho(action: Action, from: string | null, t: number) {
  const prev = lastPress.get(action);
  if (prev && (prev.from === null) !== (from === null) && t - prev.t < ECHO_MS) {
    const pad = from ?? prev.from;
    if (pad) {
      const n = (strikes.get(pad) ?? 0) + 1;
      strikes.set(pad, n);
      if (n >= ECHO_STRIKES) twins.add(pad);
    }
    return true;
  }
  lastPress.set(action, { t, from });
  return false;
}

function browserGamepad(native: boolean) {
  const held = new Map<Action, { since: number; last: number }>();

  const frame = (t: number) => {
    requestAnimationFrame(frame);
    if (!document.hasFocus()) {
      held.clear();
      return;
    }
    // Each action remembers the first pad pressing it, to blame echoes on.
    const pressed = new Map<Action, string>();
    for (const pad of navigator.getGamepads()) {
      if (!pad) continue;
      const key = `${pad.index}:${pad.id}`;
      if (twins.has(key) || nativelyHandled(pad.id, native, nativePads)) continue;
      const add = (action: Action) => {
        if (!pressed.has(action)) pressed.set(action, key);
      };
      for (const [i, action] of BUTTONS) if (pad.buttons[i]?.pressed) add(action);
      const [x = 0, y = 0] = pad.axes;
      if (x < -STICK) add("left");
      if (x > STICK) add("right");
      if (y < -STICK) add("up");
      if (y > STICK) add("down");
    }
    for (const [action, key] of pressed) {
      const state = held.get(action);
      if (!state) {
        held.set(action, { since: t, last: t });
        if (isEcho(action, key, t)) continue;
        controllerActive();
        dispatch(action, false, "pad");
      } else if (REPEATABLE.has(action) && t - state.since >= REPEAT_DELAY && t - state.last >= REPEAT_RATE) {
        state.last = t;
        if (t - (lastNative.get(action) ?? -Infinity) < NATIVE_QUIET) continue;
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
  remoteText();
  const native = isTauri ? await api.nativeGamepad().catch(() => false) : false;
  setNativeHaptics(native);
  // Also without XInput: the phone companion's presses arrive here too.
  if (isTauri) {
    onWindow<{ action: Action; repeat: boolean }>("gamepad://nav", (e) => {
      const t = performance.now();
      lastNative.set(e.action, t);
      if (!e.repeat && isEcho(e.action, null, t)) return;
      controllerActive();
      dispatch(e.action, e.repeat, "pad");
    });
    const track = (list: Controller[]) => (nativePads = list.length > 0);
    on<Controller[]>("gamepad://controllers", track);
    void api.getControllers().then(track).catch(() => {});
  }
  browserGamepad(native);
  return native;
}
