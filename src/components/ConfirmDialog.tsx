import { motion } from "motion/react";

import { Priority, useAction } from "../input/dispatcher";
import { useScreenFocus } from "./common";

interface Props {
  title: string;
  text: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}

/** Modal yes/no for irreversible actions. Focus starts on "Annulla". */
export function ConfirmDialog({ title, text, confirmLabel, onConfirm, onCancel }: Props) {
  useScreenFocus();
  useAction((action) => {
    if (action === "back") {
      onCancel();
      return true;
    }
    return !["up", "down", "left", "right", "confirm"].includes(action);
  }, Priority.BLOCKING);

  return (
    <motion.div
      className="dialog-scrim"
      data-nav-layer="dialog"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.18 }}
    >
      <motion.div
        className="dialog"
        role="alertdialog"
        aria-labelledby="dialog-title"
        initial={{ scale: 0.94, y: 12 }}
        animate={{ scale: 1, y: 0 }}
        transition={{ type: "spring", stiffness: 380, damping: 30 }}
      >
        <h2 id="dialog-title" className="dialog-title">
          {title}
        </h2>
        <p className="dialog-text">{text}</p>
        <div className="button-row dialog-actions">
          <button type="button" className="btn" data-nav data-nav-autofocus onClick={onCancel}>
            Annulla
          </button>
          <button type="button" className="btn btn-danger" data-nav onClick={onConfirm}>
            {confirmLabel}
          </button>
        </div>
      </motion.div>
    </motion.div>
  );
}
