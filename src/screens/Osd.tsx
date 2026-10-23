import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";

import { api, artUrl, on } from "../lib/api";
import type { OsdNotice } from "../lib/types";

/** Just under the backend's SHOW_FOR (osd.rs): the card fades out before the
 * window hides, and the next one starts from an empty window. */
const SHOW_FOR = 2600;

/** Runs in the small non-focusable window that pops over the game after a
 * screenshot. The backend positions, shows and hides the window; this only
 * draws the card. */
export function OsdApp() {
  const [notice, setNotice] = useState<(OsdNotice & { at: number }) | null>(null);

  useEffect(() => {
    // The first notice may be sent before this page listens: ask for it.
    void api
      .osdNotice()
      .then((n) => n && setNotice({ ...n, at: Date.now() }))
      .catch(() => {});
    return on<OsdNotice>("osd://show", (n) => setNotice({ ...n, at: Date.now() }));
  }, []);

  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), SHOW_FOR);
    return () => clearTimeout(timer);
  }, [notice]);

  return (
    <div className="osd-root">
      <AnimatePresence>
        {notice && (
          <motion.div
            key={notice.at}
            className={`osd-card ${notice.kind === "error" ? "is-error" : ""}`}
            initial={{ opacity: 0, y: 18, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0 }}
            transition={{ type: "spring", stiffness: 420, damping: 32 }}
          >
            {notice.kind === "saved" ? (
              <>
                <img className="osd-thumb" src={artUrl(notice.capture.path)} alt="" draggable={false} />
                <span className="osd-text">
                  <span className="osd-title">Schermata salvata</span>
                  <span className="osd-sub">{notice.capture.gameName} · la trovi nella pagina del gioco</span>
                </span>
              </>
            ) : (
              <>
                <span className="osd-icon" aria-hidden>
                  !
                </span>
                <span className="osd-text">
                  <span className="osd-title">Schermata non riuscita</span>
                  <span className="osd-sub">{notice.message}</span>
                </span>
              </>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
