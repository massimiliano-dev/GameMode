import { useEffect, useState } from "react";

import { isTauri, on } from "../lib/api";
import type { OsdPayload } from "../lib/types";

/** Runs in the small click-through window that pops up over a game when the volume changes. */
export function OsdApp() {
  // In the browser (`npm run dev?osd=1`) a sample is shown to style against.
  const [osd, setOsd] = useState<OsdPayload | null>(isTauri ? null : { level: 0.6, muted: false, label: null });

  useEffect(() => on<OsdPayload>("osd://show", setOsd), []);

  if (!osd) return null;
  const pct = Math.round(osd.level * 100);
  return (
    <div className={`osd ${osd.muted ? "is-muted" : ""}`}>
      <span className="osd-icon">
        <SpeakerIcon muted={osd.muted} level={osd.level} />
      </span>
      <span className="osd-body">
        <span className="osd-label">{osd.label ?? (osd.muted ? "Muto" : "Volume")}</span>
        <span className="osd-bar">
          <span className="osd-fill" style={{ width: `${osd.muted ? 0 : pct}%` }} />
        </span>
      </span>
      <span className="osd-value">{osd.muted ? "Muto" : `${pct}%`}</span>
    </div>
  );
}

function SpeakerIcon({ muted, level }: { muted: boolean; level: number }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M4 9v6h4l5 4V5L8 9H4z" fill="currentColor" stroke="none" />
      {muted ? (
        <path d="M16 9l5 6M21 9l-5 6" />
      ) : (
        <>
          {level > 0 && <path d="M16.5 9.5a3.5 3.5 0 0 1 0 5" />}
          {level > 0.5 && <path d="M19 7a7 7 0 0 1 0 10" />}
        </>
      )}
    </svg>
  );
}
