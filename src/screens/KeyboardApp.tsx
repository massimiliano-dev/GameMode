import { useEffect, useState } from "react";

import { VirtualKeyboard } from "../components/VirtualKeyboard";
import { focusInitial } from "../input/focus";
import { setupInput } from "../input/sources";
import { api, on } from "../lib/api";

/** Runs in the system keyboard window: never focused, driven by the
 * controller through the backend, typing into whatever window has the
 * focus (a store dialog, a game's chat, Wand). */
export function KeyboardApp() {
  const [openedAt, setOpenedAt] = useState(0);
  const [masked, setMasked] = useState(false);

  useEffect(() => {
    void setupInput();
    return on<{ masked: boolean }>("keyboard://open", (e) => {
      setMasked(e.masked);
      // A fresh keyboard on every open: the preview starts empty.
      setOpenedAt(Date.now());
      requestAnimationFrame(() => focusInitial());
    });
  }, []);

  return <VirtualKeyboard key={openedAt} masked={masked} onClose={() => void api.keyboardHide()} />;
}
