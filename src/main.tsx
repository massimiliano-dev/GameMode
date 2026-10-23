import "@fontsource-variable/outfit";
import "./styles.css";

import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { OsdApp } from "./screens/Osd";
import { OverlayApp } from "./screens/Overlay";

// The same bundle serves the launcher, the in-game overlay window and the
// volume indicator.
const params = new URLSearchParams(window.location.search);
const isOverlay = params.has("overlay");
const isOsd = params.has("osd");
if (isOverlay) document.documentElement.classList.add("is-overlay");
if (isOsd) document.documentElement.classList.add("is-osd");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isOsd ? <OsdApp /> : isOverlay ? <OverlayApp /> : <App />}</React.StrictMode>,
);
