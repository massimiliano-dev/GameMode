import { motion } from "motion/react";
import { useEffect, useRef, useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { onWindow } from "../lib/api";
import type { RemoteText } from "../lib/types";
import { play } from "../input/sfx";
import { Glyph, useScreenFocus } from "./common";

const LETTERS = [
  ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"],
  ["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"],
  ["a", "s", "d", "f", "g", "h", "j", "k", "l", "'"],
  ["z", "x", "c", "v", "b", "n", "m", ",", ".", "-"],
];

const SYMBOLS = [
  ["!", "@", "#", "$", "%", "&", "*", "(", ")", "="],
  ["à", "è", "é", "ì", "ò", "ù", "+", "?", "/", "\\"],
  [":", ";", '"', "_", "[", "]", "{", "}", "<", ">"],
  ["~", "|", "^", "€", "£", "°", "ç", "«", "»", "§"],
];

/** Writes into a React-controlled input as if the user typed. */
function setFieldValue(field: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  setter?.call(field, value);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

interface Props {
  field: HTMLInputElement;
  onClose: () => void;
}

/** On-screen keyboard bound to a text field, for typing with a controller. */
export function VirtualKeyboard({ field, onClose }: Props) {
  const [value, setValue] = useState(field.value);
  const [shift, setShift] = useState(false);
  const [symbols, setSymbols] = useState(false);
  const label = field.getAttribute("aria-label") ?? field.placeholder ?? "Testo";

  useScreenFocus();
  useEffect(() => setFieldValue(field, value), [field, value]);

  const type = (text: string) => {
    setValue((v) => v + text);
    if (shift) setShift(false);
  };
  const erase = () => setValue((v) => v.slice(0, -1));
  const close = () => {
    onClose();
    requestAnimationFrame(() => field.focus({ preventScroll: true }));
  };
  const done = () => {
    close();
    // Screens save on Enter (e.g. API key fields).
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  };
  // Text typed on the phone companion lands here while the keyboard is open.
  const doneRef = useRef(done);
  doneRef.current = done;
  useEffect(
    () =>
      onWindow<RemoteText>("remote://text", (e) => {
        setValue((v) => (e.backspace ? v.slice(0, -1) : v) + e.text);
        if (e.enter) requestAnimationFrame(() => doneRef.current());
      }),
    [],
  );

  useAction((action, repeat) => {
    switch (action) {
      case "back":
        if (!repeat) close();
        return true;
      case "x":
        erase();
        play("back");
        return true;
      case "y":
        type(" ");
        play("select");
        return true;
      case "lt":
        if (!repeat) setShift((s) => !s);
        return true;
      case "rt":
        if (!repeat) setSymbols((s) => !s);
        return true;
      case "menu":
        if (!repeat) done();
        return true;
      case "up":
      case "down":
      case "left":
      case "right":
      case "confirm":
        return false;
      default:
        return true;
    }
  }, Priority.BLOCKING);

  const rows = symbols ? SYMBOLS : LETTERS;
  const masked = field.type === "password" ? "•".repeat(value.length) : value;

  return (
    <motion.div
      className="osk-scrim"
      data-nav-layer="keyboard"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ duration: 0.15 }}
    >
      <motion.div
        className="osk"
        initial={{ y: 40 }}
        animate={{ y: 0 }}
        transition={{ type: "spring", stiffness: 420, damping: 36 }}
      >
        <div className="osk-preview">
          <span className="osk-label">{label}</span>
          <span className="osk-text">
            {masked}
            <span className="osk-caret" />
          </span>
        </div>
        <div className="osk-keys">
          {rows.map((row, r) => (
            <div key={r} className="osk-row">
              {row.map((key, i) => {
                const char = shift ? key.toUpperCase() : key;
                return (
                  <button
                    key={key}
                    type="button"
                    className="osk-key"
                    data-nav
                    data-nav-autofocus={(r === 1 && i === 0) || undefined}
                    onClick={() => type(char)}
                  >
                    {char}
                  </button>
                );
              })}
            </div>
          ))}
          <div className="osk-row">
            <button type="button" className={`osk-key osk-wide ${shift ? "is-on" : ""}`} data-nav onClick={() => setShift((s) => !s)}>
              ⇧ Maiusc
            </button>
            <button type="button" className={`osk-key osk-wide ${symbols ? "is-on" : ""}`} data-nav onClick={() => setSymbols((s) => !s)}>
              {symbols ? "abc" : "àè?#"}
            </button>
            <button type="button" className="osk-key osk-space" data-nav onClick={() => type(" ")}>
              Spazio
            </button>
            <button type="button" className="osk-key osk-wide" data-nav onClick={erase}>
              ⌫
            </button>
            <button type="button" className="osk-key osk-wide osk-done" data-nav onClick={done}>
              Fatto
            </button>
          </div>
        </div>
        <div className="osk-hints">
          <span className="hint"><Glyph button="X" /> Cancella</span>
          <span className="hint"><Glyph button="Y" /> Spazio</span>
          <span className="hint"><Glyph button="LT" /> Maiuscole</span>
          <span className="hint"><Glyph button="RT" /> Simboli</span>
          <span className="hint"><Glyph button="menu" /> Fatto</span>
          <span className="hint"><Glyph button="B" /> Chiudi</span>
        </div>
      </motion.div>
    </motion.div>
  );
}
