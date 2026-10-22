import "@fontsource-variable/outfit";
import "./styles.css";

import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { OverlayApp } from "./screens/Overlay";

// The same bundle serves the launcher and the in-game overlay window.
const isOverlay = new URLSearchParams(window.location.search).has("overlay");
if (isOverlay) document.documentElement.classList.add("is-overlay");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isOverlay ? <OverlayApp /> : <App />}</React.StrictMode>,
);
