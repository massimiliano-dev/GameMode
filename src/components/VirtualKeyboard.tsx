import { motion } from "motion/react";
import { useEffect, useRef, useState } from "react";

import { Priority, useAction } from "../input/dispatcher";
import { onWindow } from "../lib/api";
import type { RemoteText } from "../lib/types";
import { play } from "../input/sfx";
import { api } from "../lib/api";
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
  /** The field typed into; without one, keystrokes go to whatever window
   * has the focus (the system keyboard window). */
  field?: HTMLInputElement;
  /** Remote mode: the preview shows dots (a password field). */
  masked?: boolean;
  onClose: () => void;
}

/** On-screen keyboard for typing with a controller: into a text field of
 * the launcher, or (without a field) into the focused window of another app. */
export function VirtualKeyboard({ field, masked, onClose }: Props) {
  const [value, setValue] = useState(field?.value ?? "");
  const [shift, setShift] = useState(false);
  const [symbols, setSymbols] = useState(false);
  // Remote mode: View shows or hides the text, for passwords UI Automation
  // can't see (Steam's login).
  const [hidden, setHidden] = useState(masked ?? false);
  const label = field ? (field.getAttribute("aria-label") ?? field.placeholder ?? "Testo") : "Tastiera";

  useScreenFocus();
  useEffect(() => {
    if (field) setFieldValue(field, value);
  }, [field, value]);

  const type = (text: string) => {
    setValue((v) => v + text);
    if (!field) void api.typeText(text);
    if (shift) setShift(false);
  };
  const erase = () => {
    setValue((v) => v.slice(0, -1));
    if (!field) void api.typeKey("backspace");
  };
  const close = () => {
    onClose();
    if (field) requestAnimationFrame(() => field.focus({ preventScroll: true }));
  };
  const done = () => {
    close();
    if (field) {
      // Screens save on Enter (e.g. API key fields).
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    } else {
      void api.typeKey("enter");
    }
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
      case "view":
        if (!repeat && !field) setHidden((h) => !h);
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
  const dots = field ? field.type === "password" : hidden;
  const shown = dots ? "•".repeat(value.length) : value;

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
            {shown}
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
              {field ? "Fatto" : "Invio"}
            </button>
          </div>
        </div>
        <div className="osk-hints">
          <span className="hint"><Glyph button="X" /> Cancella</span>
          <span className="hint"><Glyph button="Y" /> Spazio</span>
          <span className="hint"><Glyph button="LT" /> Maiuscole</span>
          <span className="hint"><Glyph button="RT" /> Simboli</span>
          <span className="hint"><Glyph button="menu" /> {field ? "Fatto" : "Invio"}</span>
          {!field && <span className="hint"><Glyph button="view" /> {hidden ? "Mostra" : "Nascondi"}</span>}
          <span className="hint"><Glyph button="B" /> Chiudi</span>
        </div>
      </motion.div>
    </motion.div>
  );
}
