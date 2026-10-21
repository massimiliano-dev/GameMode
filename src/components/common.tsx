import { useEffect, useLayoutEffect, useState } from "react";

import { focusInitial } from "../input/focus";
import { STORE_LABEL } from "../lib/format";
import type { Store } from "../lib/types";
import { type Hint, useApp } from "../store/app";

export function StoreBadge({ store, compact }: { store: Store; compact?: boolean }) {
  return (
    <span className={`store-badge store-${store}`}>
      <span className="store-dot" />
      {!compact && STORE_LABEL[store]}
    </span>
  );
}

const GLYPH_TEXT: Record<Hint["button"], string> = {
  A: "A",
  B: "B",
  X: "X",
  Y: "Y",
  LB: "LB",
  RB: "RB",
  LT: "LT",
  RT: "RT",
  menu: "☰",
  view: "⧉",
};

export function Glyph({ button }: { button: Hint["button"] }) {
  return <span className={`glyph glyph-${button}`}>{GLYPH_TEXT[button]}</span>;
}

/** Sets the controller hints shown in the bottom bar while mounted. */
export function useHints(hints: Hint[], deps: unknown[] = []) {
  const setHints = useApp((s) => s.setHints);
  useEffect(() => {
    setHints(hints);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

/** Restores or sets initial focus once the screen has rendered. */
export function useScreenFocus(focusId?: string, ready = true) {
  useLayoutEffect(() => {
    if (!ready) return;
    const raf = requestAnimationFrame(() => focusInitial(focusId));
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ready]);
}

export function useClock(intervalMs = 15_000) {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs]);
  return now;
}

export function Toggle({ on }: { on: boolean }) {
  return (
    <span className={`toggle ${on ? "is-on" : ""}`} aria-hidden>
      <span className="toggle-knob" />
    </span>
  );
}
