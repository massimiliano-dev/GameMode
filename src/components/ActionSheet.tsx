import { motion } from "motion/react";

import { Priority, useAction } from "../input/dispatcher";
import { useScreenFocus } from "./common";

export interface SheetAction {
  label: string;
  hint?: string;
  danger?: boolean;
  onSelect: () => void;
}

/** A short list of secondary actions, opened from a screen's Options button. */
export function ActionSheet({ title, actions, onClose }: { title: string; actions: SheetAction[]; onClose: () => void }) {
  useScreenFocus();
  useAction((action, repeat) => {
    if ((action === "back" || action === "menu") && !repeat) {
      onClose();
      return true;
    }
    return !["up", "down", "left", "right", "confirm"].includes(action);
  }, Priority.BLOCKING);

  return (
    <motion.div
      className="dialog-scrim"
      data-nav-layer="sheet"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.15 }}
    >
      <motion.div className="dialog sheet" role="menu" aria-label={title} initial={{ y: 24 }} animate={{ y: 0 }}>
        <h2 className="dialog-title">{title}</h2>
        <div className="qm-list sheet-list">
          {actions.map((a, i) => (
            <button
              key={a.label}
              type="button"
              role="menuitem"
              className={`qm-button ${a.danger ? "is-danger" : ""}`}
              data-nav
              data-nav-autofocus={i === 0 || undefined}
              onClick={() => {
                onClose();
                a.onSelect();
              }}
            >
              {a.label}
              {a.hint && <span className="qm-sub">{a.hint}</span>}
            </button>
          ))}
        </div>
      </motion.div>
    </motion.div>
  );
}
