#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Claude Awake — a background utility that pins a laptop awake (lid closed,
//! battery ignored, Wi-Fi held up) for as long as a Claude Code session is running,
//! and shows a small pill stuck to the frontmost terminal window.
//!
//! Architecture, briefly:
//!   * this process owns the UI, the state machine, and session-level assertions;
//!   * `claude-awake-helperd` (root) owns anything that needs privilege;
//!   * they talk over a local socket with a heartbeat, so a crash here always
//!     ends with the machine's power settings restored.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, RunEvent, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_autostart::ManagerExt;

use claude_awake_lib::detect::Detector;
use claude_awake_lib::keepalive::SessionKeepalive;
use claude_awake_lib::lidwatch::LidWatcher;
use claude_awake_lib::protocol::{Guards, Profile, Request};
use claude_awake_lib::{ipc, tracker};

const OVERLAY_W: f64 = 236.0;
/// Starting height only; the webview measures its own card and reports the real
/// one. The window must hug the card exactly — any surplus is an invisible
/// rectangle that swallows clicks meant for the terminal underneath.
const OVERLAY_H_INITIAL: f64 = 46.0;
const OVERLAY_H_MIN: f64 = 32.0;
const OVERLAY_H_MAX: f64 = 400.0;
/// Inset from the terminal's top-right corner, below a typical title bar.
const OVERLAY_INSET_X: f64 = 14.0;
const OVERLAY_INSET_Y: f64 = 40.0;

const TRAY_ID: &str = "main";

const TICK: Duration = Duration::from_millis(600);
const HEARTBEAT_EVERY: Duration = Duration::from_secs(30);

// ── state ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Off,
    #[default]
    Auto,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum HelperStatus {
    Ok,
    Missing,
    Error,
}

/// Settings that survive a restart. Deliberately tiny — everything else is derived.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
struct Settings {
    mode: Mode,
    keep_display: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    mode: Mode,
    protecting: bool,
    /// A Claude Code session exists, busy or not.
    claude_active: bool,
    /// A turn is actually in flight. Only meaningful when `precise_detection`.
    claude_busy: bool,
    /// Hook events are driving Auto mode, rather than the coarse process scan.
    precise_detection: bool,
    helper: HelperStatus,
    helper_detail: String,
    guards: Guards,
    awake_secs: u64,
    attached_app: Option<String>,
    keep_display: bool,
    autostart: bool,
}

struct Core {
    settings: Settings,
    protecting: bool,
    guards: Guards,
    helper: HelperStatus,
    helper_detail: String,
    since: Option<Instant>,
    attached_app: Option<String>,
    last_beat: Instant,
    autostart: bool,
    /// Set when the profile changed while protection is already on, so the next
    /// reconcile re-sends it instead of short-circuiting on "nothing to do".
    force_reapply: bool,
}

struct Shared {
    core: Mutex<Core>,
    detector: Arc<Detector>,
    keepalive: SessionKeepalive,
    lid: LidWatcher,
    /// Reported by the overlay webview. Stored in tenths of a point so it fits an
    /// atomic and needs no lock on the hot tracking path.
    overlay_height_tenths: AtomicU32,
    /// Last tooltip pushed to the tray, so the per-tick update is a no-op unless
    /// the text actually changed.
    last_tooltip: Mutex<String>,
}

impl Shared {
    fn snapshot(&self) -> Snapshot {
        let core = self.core.lock().unwrap();
        Snapshot {
            mode: core.settings.mode,
            protecting: core.protecting,
            claude_active: self.detector.session_present(),
            claude_busy: self.detector.busy(),
            precise_detection: self.detector.precise(),
            helper: core.helper,
            helper_detail: core.helper_detail.clone(),
            guards: core.guards,
            awake_secs: core.since.map(|t| t.elapsed().as_secs()).unwrap_or(0),
            attached_app: core.attached_app.clone(),
            keep_display: core.settings.keep_display,
            autostart: core.autostart,
        }
    }
}

// ── the state machine ───────────────────────────────────────────────────────

/// Brings the machine's actual power state in line with the current mode. Safe to
/// call on every tick: the expensive helper round-trip only happens on a change.
fn reconcile(app: &AppHandle, shared: &Shared) {
    let (mode, keep_display, protecting, needs_beat, forced) = {
        let mut core = shared.core.lock().unwrap();
        let forced = std::mem::take(&mut core.force_reapply);
        (
            core.settings.mode,
            core.settings.keep_display,
            core.protecting,
            core.last_beat.elapsed() >= HEARTBEAT_EVERY,
            forced,
        )
    };

    let want = match mode {
        Mode::Off => false,
        Mode::Always => true,
        Mode::Auto => shared.detector.active(),
    };

    if want == protecting && !(forced && want) {
        if want && needs_beat {
            // Losing the heartbeat is how the helper decides we died, so this is
            // not optional bookkeeping.
            let ok = matches!(ipc::request(&Request::Heartbeat), Ok(r) if r.ok);
            let mut core = shared.core.lock().unwrap();
            core.last_beat = Instant::now();
            if !ok {
                core.helper = HelperStatus::Missing;
            }
        }
        return;
    }

    // Session-level assertions first: they need no privilege, so even a machine
    // without the helper gets idle-sleep protection immediately.
    shared.keepalive.set(want, keep_display);
    // With clamshell sleep suppressed, nothing else will darken the panel when
    // the lid shuts.
    shared.lid.set_armed(want && !keep_display);

    let request = if want {
        Request::Apply(Profile { keep_display })
    } else {
        Request::Restore
    };

    let (helper, guards, detail) = match ipc::request(&request) {
        Ok(reply) if reply.ok => (HelperStatus::Ok, reply.guards, reply.detail),
        Ok(reply) => (HelperStatus::Error, reply.guards, reply.detail),
        Err(e) => (HelperStatus::Missing, Guards::default(), e),
    };

    {
        let mut core = shared.core.lock().unwrap();
        core.protecting = want;
        core.since = if want { Some(Instant::now()) } else { None };
        core.last_beat = Instant::now();
        core.helper = helper;
        core.helper_detail = detail;
        // Without the helper only the idle-sleep half is real; do not claim a lid
        // guard we are not actually providing.
        core.guards = if want { guards } else { Guards::default() };
    }
    push_state(app, shared);
}

/// Moves the overlay onto the frontmost terminal, or hides it when there is none.
fn track_overlay(app: &AppHandle, shared: &Shared) {
    let Some(overlay) = app.get_webview_window("overlay") else {
        return;
    };
    let found = tracker::frontmost_terminal();

    let changed = {
        let mut core = shared.core.lock().unwrap();
        let name = found.as_ref().map(|w| w.app.clone());
        let changed = core.attached_app != name;
        core.attached_app = name;
        changed
    };

    match found {
        Some(win) => {
            let height = shared.overlay_height_tenths.load(Ordering::Relaxed) as f64 / 10.0;
            // Clamp so the pill never hangs off a narrow window's left edge.
            let x = (win.x + win.w - OVERLAY_W - OVERLAY_INSET_X).max(win.x + 8.0);
            let y = win.y + OVERLAY_INSET_Y;
            let _ = overlay.set_size(LogicalSize::new(OVERLAY_W, height));
            let _ = overlay.set_position(LogicalPosition::new(x, y));
            if !overlay.is_visible().unwrap_or(false) {
                let _ = overlay.show();
            }
        }
        None => {
            if overlay.is_visible().unwrap_or(false) {
                let _ = overlay.hide();
            }
        }
    }

    if changed {
        push_state(app, shared);
    }
}

fn push_state(app: &AppHandle, shared: &Shared) {
    let snapshot = shared.snapshot();
    update_tray_tooltip(app, shared, &snapshot);
    let _ = app.emit("state", snapshot);
}

/// The tray icon is the only affordance visible when no terminal is in front, so
/// it carries the current state in its tooltip.
fn update_tray_tooltip(app: &AppHandle, shared: &Shared, snapshot: &Snapshot) {
    let text = if snapshot.protecting {
        let mins = snapshot.awake_secs / 60;
        if mins >= 60 {
            format!(
                "Claude Awake — staying awake for {}h {}m",
                mins / 60,
                mins % 60
            )
        } else {
            format!("Claude Awake — staying awake for {mins}m")
        }
    } else {
        match snapshot.mode {
            Mode::Off => "Claude Awake — off".to_string(),
            _ => "Claude Awake — standing by".to_string(),
        }
    };

    let mut last = shared.last_tooltip.lock().unwrap();
    if *last == text {
        return;
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(&text));
    }
    *last = text;
}

// ── commands ────────────────────────────────────────────────────────────────

#[tauri::command]
fn get_state(shared: State<Arc<Shared>>) -> Snapshot {
    shared.snapshot()
}

#[tauri::command]
fn set_mode(app: AppHandle, shared: State<Arc<Shared>>, mode: Mode) -> Snapshot {
    {
        let mut core = shared.core.lock().unwrap();
        core.settings.mode = mode;
        save_settings(&app, core.settings);
    }
    reconcile(&app, &shared);
    let snap = shared.snapshot();
    let _ = app.emit("state", snap.clone());
    snap
}

/// The pill's single tap: flips between "not protecting" and the user's last
/// non-off mode, defaulting to automatic.
#[tauri::command]
fn toggle_protection(app: AppHandle, shared: State<Arc<Shared>>) -> Snapshot {
    let next = {
        let core = shared.core.lock().unwrap();
        if core.settings.mode == Mode::Off {
            Mode::Auto
        } else {
            Mode::Off
        }
    };
    set_mode(app, shared, next)
}

#[tauri::command]
fn set_keep_display(app: AppHandle, shared: State<Arc<Shared>>, on: bool) -> Snapshot {
    {
        let mut core = shared.core.lock().unwrap();
        core.settings.keep_display = on;
        core.force_reapply = true;
        save_settings(&app, core.settings);
    }
    reconcile(&app, &shared);
    let snap = shared.snapshot();
    let _ = app.emit("state", snap.clone());
    snap
}

#[tauri::command]
fn set_autostart(app: AppHandle, shared: State<Arc<Shared>>, on: bool) -> Snapshot {
    let manager = app.autolaunch();
    let result = if on {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(e) = result {
        eprintln!("[claude-awake] autostart: {e}");
    }
    {
        let mut core = shared.core.lock().unwrap();
        core.autostart = manager.is_enabled().unwrap_or(on);
    }
    let snap = shared.snapshot();
    let _ = app.emit("state", snap.clone());
    snap
}

/// The webview owns its own layout, so it also owns the window's height.
#[tauri::command]
fn set_overlay_height(app: AppHandle, shared: State<Arc<Shared>>, height: f64) {
    if !height.is_finite() {
        return;
    }
    let clamped = height.clamp(OVERLAY_H_MIN, OVERLAY_H_MAX);
    shared
        .overlay_height_tenths
        .store((clamped * 10.0).round() as u32, Ordering::Relaxed);
    track_overlay(&app, &shared);
}

#[tauri::command]
fn open_panel(app: AppHandle) {
    show_panel(&app);
}

/// The exact command the user must run to install the privileged helper. Built
/// from the resolved resource path so it works for a bundled app and a dev build
/// alike, and copied to the clipboard by the settings panel.
#[tauri::command]
fn install_helper_command(app: AppHandle) -> String {
    #[cfg(windows)]
    let (name, fallback) = ("install-helper.ps1", "scripts\\install-helper.ps1");
    #[cfg(not(windows))]
    let (name, fallback) = ("install-helper.sh", "scripts/install-helper.sh");

    let script = app
        .path()
        .resolve(name, tauri::path::BaseDirectory::Resource)
        .ok()
        .filter(|p| p.exists())
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| fallback.to_string());

    #[cfg(windows)]
    {
        // Must run elevated; Start-Process -Verb RunAs raises the UAC prompt so
        // the user does not have to know how to open an admin shell.
        format!(
            "Start-Process powershell -Verb RunAs -ArgumentList \
             '-ExecutionPolicy','Bypass','-File','\"{script}\"'"
        )
    }
    #[cfg(not(windows))]
    format!("sudo bash \"{script}\"")
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ── settings persistence ────────────────────────────────────────────────────

fn settings_path(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("settings.json"))
}

fn load_settings(app: &AppHandle) -> Settings {
    settings_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_settings(app: &AppHandle, settings: Settings) {
    let Some(path) = settings_path(app) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&settings) {
        let _ = std::fs::write(path, text);
    }
}

// ── windows ─────────────────────────────────────────────────────────────────

fn build_overlay(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("index.html".into()))
        .title("Claude Awake")
        .inner_size(OVERLAY_W, OVERLAY_H_INITIAL)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .minimizable(false)
        .maximizable(false)
        .closable(false)
        .visible(false)
        .focused(false)
        .build()?;

    #[cfg(target_os = "macos")]
    make_overlay_omnipresent(&window);

    Ok(window)
}

/// Lets the pill follow the user across Spaces and sit above full-screen apps —
/// without this it vanishes the moment a terminal goes full screen, which is
/// exactly when a long Claude run is most likely to be happening.
#[cfg(target_os = "macos")]
fn make_overlay_omnipresent(window: &WebviewWindow) {
    use objc2::runtime::AnyObject;

    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const STATIONARY: usize = 1 << 4;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;

    let Ok(ptr) = window.ns_window() else { return };
    let ns_window = ptr as *mut AnyObject;
    unsafe {
        let behavior = CAN_JOIN_ALL_SPACES | STATIONARY | FULL_SCREEN_AUXILIARY;
        let _: () = objc2::msg_send![ns_window, setCollectionBehavior: behavior];
        // Survive app deactivation; the pill belongs to the terminal, not to us.
        let _: () = objc2::msg_send![ns_window, setHidesOnDeactivate: false];
    }
}

fn show_panel(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("panel") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }
    let builder = WebviewWindowBuilder::new(app, "panel", WebviewUrl::App("index.html".into()))
        .title("Claude Awake")
        .inner_size(380.0, 700.0)
        .resizable(false)
        .maximizable(false)
        .skip_taskbar(true)
        .visible(true);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    match builder.build() {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(e) => eprintln!("[claude-awake] could not open panel: {e}"),
    }
}

// ── tray ────────────────────────────────────────────────────────────────────

fn build_tray(app: &AppHandle, shared: Arc<Shared>) -> tauri::Result<()> {
    let mode = shared.core.lock().unwrap().settings.mode;

    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let off = CheckMenuItem::with_id(app, "off", "Off", true, mode == Mode::Off, None::<&str>)?;
    let auto = CheckMenuItem::with_id(
        app,
        "auto",
        "Automatic",
        true,
        mode == Mode::Auto,
        None::<&str>,
    )?;
    let always = CheckMenuItem::with_id(
        app,
        "always",
        "Always on",
        true,
        mode == Mode::Always,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &off,
            &auto,
            &always,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let checks = [(Mode::Off, off), (Mode::Auto, auto), (Mode::Always, always)];
    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Claude Awake")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(move |app, event| {
            let shared: State<Arc<Shared>> = app.state();
            let next = match event.id().as_ref() {
                "settings" => {
                    show_panel(app);
                    return;
                }
                "quit" => {
                    app.exit(0);
                    return;
                }
                "off" => Mode::Off,
                "auto" => Mode::Auto,
                "always" => Mode::Always,
                _ => return,
            };
            set_mode(app.clone(), shared, next);
            for (m, item) in &checks {
                let _ = item.set_checked(*m == next);
            }
        });

    // A template image is drawn by the OS in the menu-bar's own colour, so it
    // stays legible in both light and dark menu bars.
    let builder = match tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png")) {
        Ok(icon) => builder.icon(icon).icon_as_template(true),
        Err(_) => builder,
    };
    builder.build(app)?;
    Ok(())
}

// ── entry point ─────────────────────────────────────────────────────────────

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            get_state,
            set_mode,
            toggle_protection,
            set_keep_display,
            set_autostart,
            set_overlay_height,
            open_panel,
            install_helper_command,
            quit_app,
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // A menu-bar utility, not an application: no Dock icon, no app menu.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // The whole point is that it is already running when a Claude session
            // starts, so opt in on first launch. The panel can turn it back off.
            let first_run = settings_path(&handle).is_none_or(|p| !p.exists());
            if first_run {
                if let Err(e) = handle.autolaunch().enable() {
                    eprintln!("[claude-awake] could not enable autostart: {e}");
                }
            }

            let settings = load_settings(&handle);
            if first_run {
                // Persist immediately, otherwise every launch looks like the first
                // one and would keep re-enabling autostart after the user turns it
                // off.
                save_settings(&handle, settings);
            }
            let autostart = handle.autolaunch().is_enabled().unwrap_or(false);

            // Adopt whatever the helper is already doing. After an unclean exit
            // (crash, SIGKILL, force quit) the machine can still be protected while
            // a fresh UI assumes it is idle — and if the saved mode is Off, nothing
            // would ever reconcile the two. The helper's watchdog eventually catches
            // it, but "eventually" is five minutes of a laptop that will not sleep.
            let adopted = ipc::request(&Request::Ping);
            let (protecting, guards, helper) = match &adopted {
                Ok(reply) if reply.ok => (reply.applied, reply.guards, HelperStatus::Ok),
                Ok(_) => (false, Guards::default(), HelperStatus::Error),
                Err(_) => (false, Guards::default(), HelperStatus::Missing),
            };

            let shared = Arc::new(Shared {
                core: Mutex::new(Core {
                    settings,
                    protecting,
                    guards,
                    helper,
                    helper_detail: String::new(),
                    since: protecting.then(Instant::now),
                    attached_app: None,
                    last_beat: Instant::now(),
                    autostart,
                    // Adopting the helper's state is only half of it — this process
                    // holds no session assertion yet. Forcing one reconcile makes
                    // the first tick either re-establish both halves or tear the
                    // whole thing down, depending on the mode.
                    force_reapply: protecting,
                }),
                detector: Detector::start(),
                keepalive: SessionKeepalive::new(),
                lid: LidWatcher::start(),
                overlay_height_tenths: AtomicU32::new((OVERLAY_H_INITIAL * 10.0) as u32),
                last_tooltip: Mutex::new(String::new()),
            });
            app.manage(Arc::clone(&shared));

            build_overlay(&handle)?;
            build_tray(&handle, Arc::clone(&shared))?;

            // One loop drives everything: cheap enough to poll, and it means the
            // UI can never disagree with the machine for more than one tick.
            std::thread::spawn(move || loop {
                track_overlay(&handle, &shared);
                reconcile(&handle, &shared);
                push_state(&handle, &shared);
                std::thread::sleep(TICK);
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to start Claude Awake")
        .run(|app, event| {
            // Whatever happens, hand the machine back the way we found it.
            if matches!(event, RunEvent::Exit) {
                if let Some(shared) = app.try_state::<Arc<Shared>>() {
                    shared.keepalive.set(false, false);
                }
                let _ = ipc::request(&Request::Restore);
            }
        });
}
