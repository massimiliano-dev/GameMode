// Every input source (native XInput, browser Gamepad API, keyboard) ends up
// here as an Action. Handlers registered by screens and overlays get first
// pick, highest priority first; unclaimed directions move focus and
// unclaimed confirms click the focused element.

import { useEffect, useRef } from "react";

import type { Action } from "../lib/types";
import { moveFocus } from "./focus";

export type Handler = (action: Action, repeat: boolean) => boolean | void;

/** Screens use SCREEN, overlays MODAL, app-wide shortcuts GLOBAL. */
export const Priority = { GLOBAL: -10, SCREEN: 0, MODAL: 10, BLOCKING: 20 } as const;

interface Entry {
  handler: Handler;
  priority: number;
  order: number;
}

let entries: Entry[] = [];
let counter = 0;
const listeners = new Set<(action: Action) => void>();

export function pushHandler(handler: Handler, priority: number = Priority.SCREEN): () => void {
  const entry = { handler, priority, order: counter++ };
  entries = [...entries, entry].sort((a, b) => b.priority - a.priority || b.order - a.order);
  return () => {
    entries = entries.filter((e) => e !== entry);
  };
}

/** Observers see every action (used to show the controller hints). */
export function onAnyAction(listener: (action: Action) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function dispatch(action: Action, repeat = false) {
  listeners.forEach((l) => l(action));
  for (const entry of entries) {
    if (entry.handler(action, repeat) === true) return;
  }
  switch (action) {
    case "up":
    case "down":
    case "left":
    case "right":
      moveFocus(action);
      break;
    case "confirm":
      if (!repeat) (document.activeElement as HTMLElement | null)?.click();
      break;
  }
}

/** Registers `handler` while the component is mounted (and `enabled`). */
export function useAction(handler: Handler, priority: number = Priority.SCREEN, enabled = true) {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    if (!enabled) return;
    return pushHandler((a, r) => ref.current(a, r), priority);
  }, [priority, enabled]);
}
