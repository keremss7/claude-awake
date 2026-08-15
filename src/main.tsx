import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { IN_TAURI } from "./lib/api";
import { Pill } from "./overlay/Pill";
import { Panel } from "./panel/Panel";
import "./styles.css";

const params = new URLSearchParams(window.location.search);

// Outside Tauri (plain browser, for UI work) the surface comes from the URL.
const label = IN_TAURI
  ? getCurrentWindow().label
  : (params.get("surface") ?? "panel");
const isOverlay = label === "overlay";

const root = document.documentElement;
root.dataset.surface = isOverlay ? "overlay" : "panel";

/**
 * The overlay is always dark — it floats over a terminal. The panel is a real
 * window and follows the system, live, so flipping appearance does not need a
 * restart. `?theme=` overrides it for screenshots.
 */
function applyTheme() {
  const override = params.get("theme");
  const dark =
    isOverlay ||
    override === "dark" ||
    (override !== "light" &&
      window.matchMedia("(prefers-color-scheme: dark)").matches);
  root.dataset.theme = dark ? "dark" : "light";
  root.style.colorScheme = dark ? "dark" : "light";
}

applyTheme();
window
  .matchMedia("(prefers-color-scheme: dark)")
  .addEventListener("change", applyTheme);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>{isOverlay ? <Pill /> : <Panel />}</React.StrictMode>,
);
