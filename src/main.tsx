import "@fontsource-variable/outfit";
import "./styles.css";

import React from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { KeyboardApp } from "./screens/KeyboardApp";
import { OverlayApp } from "./screens/Overlay";

// The same bundle serves the launcher, the in-game overlay window and the
// system on-screen keyboard window.
const params = new URLSearchParams(window.location.search);
const isOverlay = params.has("overlay");
const isKeyboard = params.has("keyboard");
if (isOverlay) document.documentElement.classList.add("is-overlay");
if (isKeyboard) document.documentElement.classList.add("is-keyboard");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{isOverlay ? <OverlayApp /> : isKeyboard ? <KeyboardApp /> : <App />}</React.StrictMode>,
);
