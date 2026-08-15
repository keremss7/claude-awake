import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { ClaudeMark } from "../ui/ClaudeMark";
import {
  api,
  EMPTY,
  formatDuration,
  IN_TAURI,
  onState,
  type Mode,
  type Snapshot,
} from "../lib/api";

const MODES: { id: Mode; label: string; hint: string }[] = [
  { id: "off", label: "Off", hint: "No sleep protection is applied." },
  {
    id: "auto",
    label: "Auto",
    hint: "Protection engages while Claude Code is running and lifts when it stops.",
  },
  {
    id: "always",
    label: "Always",
    hint: "The machine stays awake until you turn this off.",
  },
];

export function Panel() {
  const [s, setS] = useState<Snapshot>(EMPTY);
  const [cmd, setCmd] = useState("");
  const [copied, setCopied] = useState(false);
  const [version, setVersion] = useState("");

  useEffect(() => {
    api.getState().then(setS).catch(() => {});
    api.installHelperCommand().then(setCmd).catch(() => {});
    if (IN_TAURI) getVersion().then(setVersion).catch(() => {});
    const un = onState(setS);
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  const activeMode = MODES.find((m) => m.id === s.mode)!;

  return (
    <div className="h-full bg-clay-50 text-ink-900 dark:bg-ink-900 dark:text-clay-50 flex flex-col">
      {/* draggable title strip — the window has no title bar of its own */}
      <div
        data-tauri-drag-region
        className="h-9 shrink-0 flex items-center justify-center"
      >
        <span
          data-tauri-drag-region
          className="text-[10px] tracking-[0.16em] uppercase text-ink-900/30 dark:text-clay-50/25"
        >
          Claude Awake
        </span>
      </div>

      {/* Scrolls rather than clipping: the helper-not-installed state is the
          tallest one and its call to action must never fall off the bottom. */}
      <div className="px-5 pb-5 flex-1 min-h-0 overflow-y-auto flex flex-col gap-4">
        {/* ── hero status ───────────────────────────────────────────────── */}
        <section
          className={[
            "rounded-2xl p-4 border transition-colors",
            s.protecting
              ? "bg-ember-500/10 border-ember-500/30"
              : "bg-black/[0.03] dark:bg-white/[0.04] border-black/8 dark:border-white/8",
          ].join(" ")}
        >
          <div className="flex items-center gap-3">
            <div className="relative flex items-center justify-center">
              {s.protecting && (
                <span
                  className="absolute inset-0 rounded-full bg-ember-500/25 blur-md"
                  style={{ animation: "ca-breathe 3.2s ease-in-out infinite" }}
                />
              )}
              <ClaudeMark
                size={28}
                spin={s.protecting}
                className={
                  s.protecting
                    ? "text-ember-500 relative"
                    : "text-ink-900/25 dark:text-clay-50/20 relative"
                }
              />
            </div>
            <div className="min-w-0">
              <div className="text-[15px] font-semibold tracking-[-0.015em]">
                {s.protecting ? "Machine is staying awake" : "Sleep allowed"}
              </div>
              <div className="text-[11.5px] text-ink-900/45 dark:text-clay-50/40 mt-0.5 tabular-nums">
                {s.protecting
                  ? `for ${formatDuration(s.awakeSecs)}`
                  : activeMode.hint}
              </div>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-2 mt-4">
            <Guard
              label="Runs with lid closed"
              ok={s.guards.lid}
              detail="Clamshell sleep disabled"
            />
            <Guard
              label="Battery state ignored"
              ok={s.guards.battery}
              detail="Low-power mode and battery sleep are off"
            />
            <Guard
              label="Wi-Fi stays up"
              ok={s.guards.wifi}
              detail="Wake-on-network and TCP keepalive armed"
            />
            <Guard
              label="Display stays on"
              ok={s.guards.display}
              detail={
                s.keepDisplay
                  ? "Held on by request"
                  : "The display is allowed to sleep"
              }
            />
          </div>

          {s.helper === "error" && s.helperDetail && (
            <p className="text-[10.5px] leading-relaxed text-ember-600 dark:text-ember-300 mt-3">
              {s.helperDetail}
            </p>
          )}
        </section>

        {/* ── mode picker ───────────────────────────────────────────────── */}
        <section>
          <Label>Mode</Label>
          <div className="grid grid-cols-3 gap-1 p-1 rounded-xl bg-black/[0.04] dark:bg-white/[0.05]">
            {MODES.map((m) => (
              <button
                key={m.id}
                onClick={() => api.setMode(m.id).then(setS).catch(() => {})}
                className={[
                  "rounded-lg py-1.5 text-[12px] font-medium transition-all duration-200",
                  s.mode === m.id
                    ? "bg-white dark:bg-ink-700 shadow-sm text-ink-900 dark:text-clay-50"
                    : "text-ink-900/45 dark:text-clay-50/40 hover:text-ink-900/70 dark:hover:text-clay-50/70",
                ].join(" ")}
              >
                {m.label}
              </button>
            ))}
          </div>
          <p className="text-[11px] leading-relaxed text-ink-900/40 dark:text-clay-50/35 mt-2">
            {activeMode.hint}
          </p>
        </section>

        {/* ── options ───────────────────────────────────────────────────── */}
        <section>
          <Label>Options</Label>
          <div className="rounded-xl border border-black/8 dark:border-white/8 divide-y divide-black/6 dark:divide-white/6 overflow-hidden">
            <Row
              title="Keep the display on"
              sub="Leave this off: with the lid shut there is nothing to look at, and holding the panel on costs battery."
              on={s.keepDisplay}
              onChange={(v) => api.setKeepDisplay(v).then(setS).catch(() => {})}
            />
            <Row
              title="Launch at login"
              sub="Starts quietly in the background when you sign in."
              on={s.autostart}
              onChange={(v) => api.setAutostart(v).then(setS).catch(() => {})}
            />
          </div>
        </section>

        {/* ── helper ────────────────────────────────────────────────────── */}
        <section>
          <Label>Privileged helper</Label>
          {s.helper === "ok" ? (
            <div className="flex items-center gap-2 rounded-xl border border-black/8 dark:border-white/8 px-3 py-2.5">
              <span className="w-1.5 h-1.5 rounded-full bg-emerald-500 shrink-0" />
              <span className="text-[12px] text-ink-900/65 dark:text-clay-50/60">
                Installed and running
              </span>
            </div>
          ) : (
            <div className="rounded-xl border border-ember-500/30 bg-ember-500/8 p-3">
              <p className="text-[11.5px] leading-relaxed text-ink-900/70 dark:text-clay-50/70">
                Staying awake with the lid closed needs administrator rights.
                Install once and you will never be asked again.
              </p>
              {s.helperDetail && (
                <p className="text-[10.5px] text-ember-600 dark:text-ember-400 mt-1.5">
                  {s.helperDetail}
                </p>
              )}
              <button
                onClick={() => {
                  navigator.clipboard.writeText(cmd);
                  setCopied(true);
                  window.setTimeout(() => setCopied(false), 1600);
                }}
                className="mt-2.5 w-full rounded-lg bg-ink-900 dark:bg-clay-50 text-clay-50 dark:text-ink-900 text-[11.5px] font-medium py-2 hover:opacity-90 transition-opacity"
              >
                {copied ? "Copied — paste it in a terminal" : "Copy setup command"}
              </button>
            </div>
          )}
        </section>

        <div className="flex-1" />

        <div className="flex items-center justify-between text-[11px] pt-1">
          <span className="text-ink-900/25 dark:text-clay-50/20 tabular-nums">
            {version && `v${version}`}
          </span>
          <button
            onClick={() => api.quit().catch(() => {})}
            className="text-ink-900/35 dark:text-clay-50/30 hover:text-ink-900/70 dark:hover:text-clay-50/60 transition-colors"
          >
            Quit Claude Awake
          </button>
        </div>
      </div>
    </div>
  );
}

function Label({ children }: { children: React.ReactNode }) {
  return (
    <h2 className="text-[10px] font-semibold tracking-[0.13em] uppercase text-ink-900/35 dark:text-clay-50/30 mb-2">
      {children}
    </h2>
  );
}

function Guard({
  label,
  ok,
  detail,
}: {
  label: string;
  ok: boolean;
  detail: string;
}) {
  return (
    <div
      title={detail}
      className={[
        "rounded-xl px-2.5 py-2 border",
        ok
          ? "bg-ember-500/10 border-ember-500/25"
          : "bg-black/[0.02] dark:bg-white/[0.03] border-black/6 dark:border-white/6",
      ].join(" ")}
    >
      <div className="flex items-center gap-1.5">
        <span
          className={[
            "w-1.5 h-1.5 rounded-full shrink-0",
            ok ? "bg-ember-500" : "bg-ink-900/15 dark:bg-clay-50/15",
          ].join(" ")}
        />
        <span
          className={[
            "text-[10.5px] font-medium leading-tight",
            ok
              ? "text-ember-600 dark:text-ember-300"
              : "text-ink-900/35 dark:text-clay-50/30",
          ].join(" ")}
        >
          {label}
        </span>
      </div>
    </div>
  );
}

function Row({
  title,
  sub,
  on,
  onChange,
}: {
  title: string;
  sub: string;
  on: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <div className="flex items-start gap-3 px-3 py-2.5">
      <div className="min-w-0 flex-1">
        <div className="text-[12.5px] font-medium">{title}</div>
        <div className="text-[10.5px] leading-relaxed text-ink-900/40 dark:text-clay-50/35 mt-0.5">
          {sub}
        </div>
      </div>
      <button
        onClick={() => onChange(!on)}
        aria-pressed={on}
        aria-label={title}
        className={[
          "relative shrink-0 rounded-full transition-colors duration-200 mt-0.5",
          on ? "bg-ember-500" : "bg-black/12 dark:bg-white/15",
        ].join(" ")}
        style={{ width: 34, height: 19 }}
      >
        <span
          className="absolute top-[2px] rounded-full bg-white transition-all duration-200"
          style={{
            width: 15,
            height: 15,
            left: on ? 17 : 2,
            boxShadow: "0 1px 2px rgba(0,0,0,.3)",
          }}
        />
      </button>
    </div>
  );
}
