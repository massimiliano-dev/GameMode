import "@fontsource-variable/outfit";
import "./styles.css";

import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { api } from "./lib/api";
import { OverlayApp } from "./screens/Overlay";
import { SafeMode } from "./screens/SafeMode";

// The same bundle serves the launcher and the in-game overlay window.
const isOverlay = new URLSearchParams(window.location.search).has("overlay");
if (isOverlay) document.documentElement.classList.add("is-overlay");

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);
const renderApp = () => root.render(<React.StrictMode><App /></React.StrictMode>);

if (isOverlay) {
  root.render(<React.StrictMode><OverlayApp /></React.StrictMode>);
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
