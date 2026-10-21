// Spatial focus navigation. Focusable elements carry `data-nav`; the topmost
// `data-nav-layer` (modals render last) confines movement. Containers marked
// `data-nav-group` remember their last focused child, so moving back into a
// row lands where you left it.

import type { Direction } from "../lib/types";

const FOCUSABLE = "[data-nav]:not([disabled])";
const memory = new Map<string, HTMLElement>();

export function activeLayer(): HTMLElement {
  const layers = document.querySelectorAll<HTMLElement>("[data-nav-layer]");
  return layers[layers.length - 1] ?? document.body;
}

function isVisible(el: HTMLElement) {
  const r = el.getBoundingClientRect();
  return r.width > 0 && r.height > 0;
}

function focusables(layer: HTMLElement): HTMLElement[] {
  return [...layer.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(isVisible);
}

function current(layer: HTMLElement): HTMLElement | null {
  const el = document.activeElement as HTMLElement | null;
  return el && layer.contains(el) && el.matches(FOCUSABLE) ? el : null;
}

function groupOf(el: HTMLElement) {
  return el.closest<HTMLElement>("[data-nav-group]");
}

export function focusElement(el: HTMLElement) {
  el.focus({ preventScroll: true });
  el.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  const group = groupOf(el);
  if (group?.dataset.navGroup) memory.set(group.dataset.navGroup, el);
}

/** Focuses `id` (a `data-nav-id`) if present, else the layer's autofocus or first element. */
export function focusInitial(id?: string) {
  const layer = activeLayer();
  const byId = id ? layer.querySelector<HTMLElement>(`[data-nav-id="${CSS.escape(id)}"]`) : null;
  const target =
    (byId && isVisible(byId) ? byId : null) ??
    layer.querySelector<HTMLElement>("[data-nav-autofocus]") ??
    focusables(layer)[0];
  if (target) focusElement(target);
}

/** Keeps focus inside the active layer, e.g. after a modal opens or an element disappears. */
export function ensureFocus() {
  const layer = activeLayer();
  if (!current(layer)) focusInitial();
}

export function currentFocusId(): string | undefined {
  return (document.activeElement as HTMLElement | null)?.dataset?.navId;
}

export function moveFocus(dir: Direction): boolean {
  const layer = activeLayer();
  const from = current(layer);
  if (!from) {
    focusInitial();
    return true;
  }

  // Sliders and similar widgets take left/right for themselves.
  const horizontal = dir === "left" || dir === "right";
  if (horizontal && from.dataset.navCapture === "x") {
    from.dispatchEvent(new CustomEvent("navadjust", { detail: dir === "left" ? -1 : 1 }));
    return true;
  }

  const r = from.getBoundingClientRect();
  const cx = r.left + r.width / 2;
  const cy = r.top + r.height / 2;
  let best: HTMLElement | null = null;
  let bestScore = Infinity;

  for (const el of focusables(layer)) {
    if (el === from) continue;
    const c = el.getBoundingClientRect();
    const ccx = c.left + c.width / 2;
    const ccy = c.top + c.height / 2;
    let primary: number;
    let secondary: number;
    if (horizontal) {
      const ahead = dir === "right" ? ccx - cx : cx - ccx;
      // Stay on the same visual row.
      const overlap = Math.min(r.bottom, c.bottom) - Math.max(r.top, c.top);
      if (ahead <= 1 || overlap <= 0) continue;
      primary = dir === "right" ? Math.max(0, c.left - r.right) : Math.max(0, r.left - c.right);
      secondary = Math.abs(ccy - cy);
    } else {
      const ahead = dir === "down" ? ccy - cy : cy - ccy;
      if (ahead <= 1) continue;
      primary = dir === "down" ? Math.max(0, c.top - r.bottom) : Math.max(0, r.top - c.bottom);
      const gap = Math.max(0, c.left - r.right, r.left - c.right);
      secondary = gap + Math.abs(ccx - cx) * 0.25;
    }
    const score = primary + secondary * 3;
    if (score < bestScore) {
      bestScore = score;
      best = el;
    }
  }
  if (!best) return false;

  // Entering another group: resume where the user left it.
  const target = groupOf(best);
  if (target && target !== groupOf(from)) {
    const remembered = memory.get(target.dataset.navGroup ?? "");
    if (remembered && target.contains(remembered) && isVisible(remembered)) best = remembered;
  }
  focusElement(best);
  return true;
}

export function forgetGroup(name: string) {
  memory.delete(name);
}
