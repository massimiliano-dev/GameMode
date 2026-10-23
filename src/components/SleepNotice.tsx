import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";

import { useApp } from "../store/app";

/** Warns that the idle timer is about to put the PC to sleep; any input cancels it. */
export function SleepNotice() {
  const sleepSoon = useApp((s) => s.sleepSoon);
  const [left, setLeft] = useState(0);

  useEffect(() => {
    if (sleepSoon === null) return;
    const until = Date.now() + sleepSoon * 1000;
    const tick = () => setLeft(Math.max(0, Math.ceil((until - Date.now()) / 1000)));
    tick();
    const timer = setInterval(tick, 1000);
    return () => clearInterval(timer);
  }, [sleepSoon]);

  return (
    <AnimatePresence>
      {sleepSoon !== null && (
        <motion.div
          key="sleep"
          className="sleep-notice"
          role="status"
          initial={{ opacity: 0, y: -12 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -8 }}
          transition={{ duration: 0.22 }}
        >
          <span className="sleep-notice-title">Sospensione tra {left} s</span>
          <span className="sleep-notice-text">Muovi il controller o premi un tasto per restare acceso</span>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
