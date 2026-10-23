import { useEffect, useLayoutEffect, useState } from "react";

import { focusInitial } from "../input/focus";
import { STORE_LABEL } from "../lib/format";
import type { PadKind, Store } from "../lib/types";
import { type Hint, useApp } from "../store/app";

export function StoreBadge({ store, compact }: { store: Store; compact?: boolean }) {
  return (
    <span className={`store-badge store-${store}`}>
      <span className="store-dot" />
      {!compact && STORE_LABEL[store]}
    </span>
  );
}

const XBOX_GLYPHS: Record<Hint["button"], string> = {
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

/** Hint labels per pad family; positions are the same, labels aren't. */
const GLYPH_TEXT: Record<PadKind, Record<Hint["button"], string>> = {
  xbox: XBOX_GLYPHS,
  playstation: { A: "✕", B: "○", X: "□", Y: "△", LB: "L1", RB: "R1", LT: "L2", RT: "R2", menu: "☰", view: "⧉" },
  nintendo: { A: "B", B: "A", X: "Y", Y: "X", LB: "L", RB: "R", LT: "ZL", RT: "ZR", menu: "+", view: "−" },
};

/** The layout of the first connected pad decides the glyphs everywhere. */
export function usePadKind(): PadKind {
  return useApp((s) => s.controllers[0]?.kind ?? "xbox");
}

export function Glyph({ button }: { button: Hint["button"] }) {
  const kind = usePadKind();
  return <span className={`glyph glyph-${button} glyph-${kind}`}>{GLYPH_TEXT[kind][button]}</span>;
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
