import "@fontsource-variable/outfit";
import "./styles.css";

import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { api } from "./lib/api";
import { KeyboardApp } from "./screens/KeyboardApp";
import { OsdApp } from "./screens/Osd";
import { OverlayApp } from "./screens/Overlay";
import { SafeMode } from "./screens/SafeMode";

// The same bundle serves the launcher, the in-game overlay window, the
// volume indicator and the system on-screen keyboard window.
const params = new URLSearchParams(window.location.search);
const isOverlay = params.has("overlay");
const isOsd = params.has("osd");
const isKeyboard = params.has("keyboard");
if (isOverlay) document.documentElement.classList.add("is-overlay");
if (isOsd) document.documentElement.classList.add("is-osd");
if (isKeyboard) document.documentElement.classList.add("is-keyboard");

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);
const renderApp = () => root.render(<React.StrictMode><App /></React.StrictMode>);

if (isOsd) {
  root.render(<React.StrictMode><OsdApp /></React.StrictMode>);
} else if (isOverlay) {
  root.render(<React.StrictMode><OverlayApp /></React.StrictMode>);
} else if (isKeyboard) {
  root.render(<React.StrictMode><KeyboardApp /></React.StrictMode>);
} else {
  // Safe mode (the watchdog gave up restarting GameMode) shows the recovery
  // screen first; "Continua comunque" then opens the launcher as usual.
  void api
    .startupInfo()
    .catch(() => null)
    .then((info) => {
      if (info?.safeMode) {
        root.render(
          <React.StrictMode>
            <SafeMode info={info} onContinue={renderApp} />
          </React.StrictMode>,
        );
      } else {
        renderApp();
      }
    });
}
