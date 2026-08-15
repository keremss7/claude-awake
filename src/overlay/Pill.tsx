import { useEffect, useRef, useState } from "react";
import { ClaudeMark } from "../ui/ClaudeMark";
import {
  api,
  EMPTY,
  formatDuration,
  IN_TAURI,
  onState,
  type Snapshot,
} from "../lib/api";

/** Must match OVERLAY_H_INITIAL in src-tauri/src/main.rs. */
const COLLAPSED_H = 46;

export function Pill() {
  const [s, setS] = useState<Snapshot>(EMPTY);
  // In browser preview mode the hover state is not reachable, so allow the URL
  // to pin it open for screenshots.
  const [expanded, setExpanded] = useState(
    !IN_TAURI && new URLSearchParams(window.location.search).has("expanded"),
  );
  const [pressed, setPressed] = useState(false);
  const hoverTimer = useRef<number | null>(null);
  const card = useRef<HTMLDivElement>(null);

  useEffect(() => {
    api.getState().then(setS).catch(() => {});
    const un = onState(setS);
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  // The native window follows the card, not the other way round: whatever the
  // layout ends up being, the window is exactly that tall and no taller.
  //
  // Zero-height readings are dropped deliberately. The card and the window size
  // each other, so one bogus measurement (StrictMode's double mount produces
  // one) would shrink the window, which shrinks the card, which reports the
  // smaller height — and the pill stays collapsed forever.
  useEffect(() => {
    const el = card.current;
    if (!el) return;
    const observer = new ResizeObserver(([entry]) => {
      const h = entry.borderBoxSize?.[0]?.blockSize ?? entry.contentRect.height;
      if (h > 0) api.setOverlayHeight(h).catch(() => {});
    });
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // Debounced so a cursor sweeping past does not thrash the window size.
  function hover(on: boolean) {
    if (hoverTimer.current !== null) window.clearTimeout(hoverTimer.current);
    hoverTimer.current = window.setTimeout(() => setExpanded(on), on ? 90 : 220);
  }

  const on = s.protecting;
  const armed = s.mode !== "off";

  return (
    <div
      className="h-full w-full flex flex-col items-end"
      onMouseEnter={() => hover(true)}
      onMouseLeave={() => hover(false)}
      onContextMenu={(e) => {
        e.preventDefault();
        api.openPanel().catch(() => {});
      }}
    >
      <div
        ref={card}
        className={[
          // shrink-0 is load-bearing: without it the flex parent squeezes the
          // card to the window height and the sizing loop can never recover.
          "w-full shrink-0 rounded-[15px] overflow-hidden",
          "border backdrop-blur-xl transition-colors duration-300",
          on
            ? "bg-ink-900/85 border-ember-500/35"
            : "bg-ink-900/72 border-white/10",
        ].join(" ")}
        style={{
          boxShadow: on
            ? "0 6px 22px -6px rgba(0,0,0,.65), 0 0 0 1px rgba(217,119,87,.10), inset 0 1px 0 rgba(255,255,255,.06)"
            : "0 6px 22px -8px rgba(0,0,0,.6), inset 0 1px 0 rgba(255,255,255,.05)",
        }}
      >
        {/* ── header row: mark · label · toggle ─────────────────────────── */}
        <div
          className="flex items-center gap-2.5 px-3"
          style={{ height: COLLAPSED_H }}
        >
          <div className="relative flex items-center justify-center shrink-0">
            {on && (
              <span
                className="absolute inset-0 rounded-full bg-ember-500/25 blur-[7px]"
                style={{ animation: "ca-breathe 3.2s ease-in-out infinite" }}
              />
            )}
            <ClaudeMark
              size={17}
              spin={on}
              className={
                on ? "text-ember-500 relative" : "text-clay-400/45 relative"
              }
            />
          </div>

          <div className="min-w-0 flex-1 leading-none">
            <div
              className={[
                "text-[12.5px] font-medium tracking-[-0.01em] truncate",
                on ? "text-clay-50" : "text-clay-400/75",
              ].join(" ")}
            >
              {on ? "Staying awake" : armed ? "Standing by" : "Off"}
            </div>
            <div className="text-[10.5px] text-clay-400/55 truncate mt-[3px] tabular-nums">
              {on
                ? formatDuration(s.awakeSecs)
                : s.mode === "auto"
                  ? "waiting for Claude"
                  : "no protection"}
              {s.claudeActive && (
                <span className="text-ember-400/80"> · claude running</span>
              )}
            </div>
          </div>

          <button
            onClick={() => {
              setPressed(true);
              window.setTimeout(() => setPressed(false), 160);
              api.toggle().then(setS).catch(() => {});
            }}
            aria-label={armed ? "Turn protection off" : "Turn protection on"}
            className={[
              "relative shrink-0 rounded-full transition-all duration-250 outline-none",
              "focus-visible:ring-2 focus-visible:ring-ember-500/60",
              armed ? "bg-ember-500" : "bg-white/12",
              pressed ? "scale-95" : "",
            ].join(" ")}
            style={{ width: 38, height: 21 }}
          >
            <span
              className="absolute top-[2.5px] rounded-full bg-white transition-all duration-250"
              style={{
                width: 16,
                height: 16,
                left: armed ? 19.5 : 2.5,
                boxShadow: "0 1px 3px rgba(0,0,0,.35)",
              }}
            />
          </button>
        </div>

        {/* ── hover detail ──────────────────────────────────────────────── */}
        {expanded && (
          <div className="ca-rise px-3 pb-2.5 -mt-0.5">
            <div className="h-px bg-white/8 mb-2.5" />
            <div className="grid grid-cols-3 gap-1.5">
              <Guard label="Lid" ok={s.guards.lid} />
              <Guard label="Battery" ok={s.guards.battery} />
              <Guard label="Wi-Fi" ok={s.guards.wifi} />
            </div>
            <div className="flex items-center justify-between mt-2.5 text-[10px]">
              <span className="text-clay-400/45 truncate">
                {s.attachedApp ?? "no terminal"}
              </span>
              {s.helper === "ok" ? (
                <button
                  onClick={() => api.openPanel().catch(() => {})}
                  className="text-clay-400/55 hover:text-clay-50 transition-colors"
                >
                  settings
                </button>
              ) : (
                <button
                  onClick={() => api.openPanel().catch(() => {})}
                  className="text-ember-400 hover:text-ember-300 transition-colors font-medium"
                >
                  setup required
                </button>
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function Guard({ label, ok }: { label: string; ok: boolean }) {
  return (
    <div
      className={[
        "rounded-[7px] px-1.5 py-1 flex items-center gap-1 border",
        ok
          ? "bg-ember-500/10 border-ember-500/25"
          : "bg-white/[0.03] border-white/8",
      ].join(" ")}
    >
      <span
        className={[
          "w-1 h-1 rounded-full shrink-0",
          ok ? "bg-ember-500" : "bg-clay-400/30",
        ].join(" ")}
      />
      <span
        className={[
          "text-[9.5px] tracking-wide truncate",
          ok ? "text-ember-300" : "text-clay-400/45",
        ].join(" ")}
      >
        {label}
      </span>
    </div>
  );
}
