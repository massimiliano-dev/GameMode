import "@fontsource-variable/outfit";
import "./styles.css";

import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { api, signalAppReady } from "./lib/api";
import { CaptureOsdApp } from "./screens/CaptureOsd";
import { KeyboardApp } from "./screens/KeyboardApp";
import { OsdApp } from "./screens/Osd";
import { OverlayApp } from "./screens/Overlay";
import { SafeMode } from "./screens/SafeMode";

// The same bundle serves the launcher, the in-game overlay window, the
// volume indicator, the screenshot card and the system on-screen keyboard
// window.
const params = new URLSearchParams(window.location.search);
const isOverlay = params.has("overlay");
const isOsd = params.has("osd");
const isKeyboard = params.has("keyboard");
const isCaptureOsd = params.has("capture-osd");
if (isOverlay) document.documentElement.classList.add("is-overlay");
if (isOsd) document.documentElement.classList.add("is-osd");
if (isKeyboard) document.documentElement.classList.add("is-keyboard");
if (isCaptureOsd) document.documentElement.classList.add("is-overlay", "is-capture-osd");

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);
const renderApp = () => root.render(<React.StrictMode><App /></React.StrictMode>);

if (isOsd) {
  root.render(<React.StrictMode><OsdApp /></React.StrictMode>);
} else if (isOverlay) {
  root.render(<React.StrictMode><OverlayApp /></React.StrictMode>);
} else if (isKeyboard) {
  root.render(<React.StrictMode><KeyboardApp /></React.StrictMode>);
} else if (isCaptureOsd) {
  root.render(<React.StrictMode><CaptureOsdApp /></React.StrictMode>);
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
        signalAppReady();
      } else {
        renderApp();
      }
    });
}
