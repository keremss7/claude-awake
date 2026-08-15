import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Mode = "off" | "auto" | "always";
export type HelperStatus = "ok" | "missing" | "error";

export interface Guards {
  /** Lid can be closed without the machine sleeping. */
  lid: boolean;
  /** Battery level / low-power transitions are ignored. */
  battery: boolean;
  /** Wi-Fi radio is pinned awake and network wake is armed. */
  wifi: boolean;
  /** Display is additionally held on (opt-in, off by default). */
  display: boolean;
}

export interface Snapshot {
  mode: Mode;
  /** True when sleep is actually being blocked right now. */
  protecting: boolean;
  /** A Claude Code session exists, busy or not. */
  claudeActive: boolean;
  /** A turn is actually in flight. Only meaningful when `preciseDetection`. */
  claudeBusy: boolean;
  /** Hook events drive Auto mode, rather than the coarse process scan. */
  preciseDetection: boolean;
  helper: HelperStatus;
  helperDetail: string;
  guards: Guards;
  /** Seconds the current protection window has been held. */
  awakeSecs: number;
  /** Terminal app the overlay is currently pinned to, if any. */
  attachedApp: string | null;
  keepDisplay: boolean;
  autostart: boolean;
}

export const EMPTY: Snapshot = {
  mode: "auto",
  protecting: false,
  claudeActive: false,
  claudeBusy: false,
  preciseDetection: false,
  helper: "missing",
  helperDetail: "",
  guards: { lid: false, battery: false, wifi: false, display: false },
  awakeSecs: 0,
  attachedApp: null,
  keepDisplay: false,
  autostart: false,
};

/** False when the page is opened in a plain browser for UI work. */
export const IN_TAURI =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/**
 * Standalone browser mode exists so the pill and panel can be iterated on (and
 * screenshotted) without rebuilding the Rust app. State comes from the query
 * string, e.g. `?surface=overlay&protecting=1&claude=1`.
 */
function mockSnapshot(): Snapshot {
  const q = new URLSearchParams(window.location.search);
  const flag = (k: string, fallback = false) =>
    q.has(k) ? q.get(k) !== "0" : fallback;
  const protecting = flag("protecting");
  return {
    ...EMPTY,
    mode: (q.get("mode") as Mode) ?? (protecting ? "auto" : "off"),
    protecting,
    claudeActive: flag("claude"),
    claudeBusy: flag("busy"),
    preciseDetection: flag("precise"),
    helper: (q.get("helper") as HelperStatus) ?? (protecting ? "ok" : "missing"),
    helperDetail: q.get("detail") ?? "",
    guards: {
      lid: protecting,
      battery: protecting,
      wifi: protecting,
      display: protecting && flag("display"),
    },
    awakeSecs: Number(q.get("secs") ?? (protecting ? 8040 : 0)),
    attachedApp: q.get("app") ?? (protecting ? "Ghostty" : null),
    keepDisplay: flag("display"),
    autostart: flag("autostart", true),
  };
}

const noop = async () => mockSnapshot();

export const api = IN_TAURI
  ? {
      getState: () => invoke<Snapshot>("get_state"),
      setMode: (mode: Mode) => invoke<Snapshot>("set_mode", { mode }),
      toggle: () => invoke<Snapshot>("toggle_protection"),
      setKeepDisplay: (on: boolean) =>
        invoke<Snapshot>("set_keep_display", { on }),
      setAutostart: (on: boolean) => invoke<Snapshot>("set_autostart", { on }),
      openPanel: () => invoke<void>("open_panel"),
      /**
       * Keeps the native window exactly as tall as the card. Surplus window is an
       * invisible click-trap over the terminal, so this is not cosmetic.
       */
      setOverlayHeight: (height: number) =>
        invoke<void>("set_overlay_height", { height }),
      installHelperCommand: () => invoke<string>("install_helper_command"),
      quit: () => invoke<void>("quit_app"),
    }
  : {
      getState: noop,
      setMode: noop,
      toggle: noop,
      setKeepDisplay: noop,
      setAutostart: noop,
      openPanel: async () => {},
      setOverlayHeight: async () => {},
      installHelperCommand: async () => "sudo bash scripts/install-helper.sh",
      quit: async () => {},
    };

/** Subscribes to backend state pushes. Returns an unlisten function. */
export function onState(cb: (s: Snapshot) => void) {
  if (!IN_TAURI) return Promise.resolve(() => {});
  return listen<Snapshot>("state", (e) => cb(e.payload));
}

export function formatDuration(totalSecs: number): string {
  const s = Math.max(0, Math.floor(totalSecs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m`;
  return `${s}s`;
}
