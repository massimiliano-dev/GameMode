// Text from the phone companion's keyboard, typed into the focused field.

import { onWindow } from "../lib/api";
import type { RemoteText } from "../lib/types";

/** Writes into a React-controlled input as if the user typed. */
function setFieldValue(field: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const proto = field instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")?.set?.call(field, value);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

/** Listens for `remote://text`; the virtual keyboard takes it while open. */
export function remoteText() {
  onWindow<RemoteText>("remote://text", (e) => {
    if (document.querySelector(".osk")) return;
    const el = document.activeElement;
    if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) return;
    const value = (e.backspace ? el.value.slice(0, -1) : el.value) + e.text;
    if (value !== el.value) setFieldValue(el, value);
    if (e.enter) el.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  });
}
