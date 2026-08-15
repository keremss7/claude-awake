//! The privileged daemon.
//!
//! Runs as root (LaunchDaemon on macOS) or LocalSystem (Windows Service) and does
//! exactly four things: snapshot power settings, apply a fixed profile, roll it
//! back, and roll it back on its own if the UI stops checking in.
//!
//! It never executes anything the caller sends — [`Request`] is the whole
//! vocabulary, and every value written to the system comes from a compile-time
//! constant. The transport is deliberately dumb: one line in, one line out.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::power::{self, Baseline};
use crate::protocol::{
    Guards, Profile, Reply, Request, BASELINE_PATH, HEARTBEAT_TIMEOUT_SECS, PROTOCOL_VERSION,
};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

const WATCHDOG_TICK: Duration = Duration::from_secs(20);

/// Set once the transport should wind down (service stop, signal). The accept
/// loop checks it between connections.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

pub fn shutdown_requested() -> bool {
    SHUTDOWN.load(Ordering::SeqCst)
}

pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// Entry point. Blocks until the transport stops.
pub fn run() {
    #[cfg(unix)]
    unix::run();
    #[cfg(windows)]
    windows::run();
    #[cfg(not(any(unix, windows)))]
    eprintln!("[claude-awake] no helper transport for this platform");
}

pub struct Helper {
    applied: bool,
    guards: Guards,
    detail: String,
    last_beat: Instant,
}

impl Default for Helper {
    fn default() -> Self {
        Helper {
            applied: false,
            guards: Guards::default(),
            detail: String::new(),
            last_beat: Instant::now(),
        }
    }
}

impl Helper {
    fn apply(&mut self, profile: Profile) -> Reply {
        let baseline = match load_or_capture_baseline() {
            Ok(b) => b,
            Err(e) => return Reply::err(e),
        };
        match power::apply(&baseline, profile) {
            Ok(outcome) => {
                self.applied = true;
                self.guards = outcome.guards;
                self.detail = outcome.warnings.join("; ");
                self.last_beat = Instant::now();
                self.reply(true)
            }
            Err(e) => Reply::err(e),
        }
    }

    fn restore(&mut self) -> Reply {
        let result = match read_baseline() {
            Ok(Some(baseline)) => power::restore(&baseline),
            // Nothing to undo is a success, not an error: the UI restores on every
            // shutdown path and must not see spurious failures.
            Ok(None) => Ok(()),
            Err(e) => Err(e),
        };
        match result {
            Ok(()) => {
                clear_baseline();
                self.applied = false;
                self.guards = Guards::default();
                self.detail.clear();
                self.reply(true)
            }
            Err(e) => Reply::err(e),
        }
    }

    fn reply(&self, ok: bool) -> Reply {
        Reply {
            ok,
            version: PROTOCOL_VERSION,
            applied: self.applied,
            guards: self.guards,
            detail: self.detail.clone(),
        }
    }
}

/// Parses one request line and returns the response line (without a newline).
pub fn handle_line(helper: &Mutex<Helper>, line: &str) -> String {
    let reply = match serde_json::from_str::<Request>(line.trim()) {
        Ok(request) => {
            let mut h = helper.lock().unwrap();
            h.last_beat = Instant::now();
            match request {
                Request::Ping | Request::Heartbeat => h.reply(true),
                Request::Apply(profile) => h.apply(profile),
                Request::Restore => h.restore(),
            }
        }
        Err(e) => Reply::err(format!("unrecognised request: {e}")),
    };
    serde_json::to_string(&reply).unwrap_or_else(|_| {
        r#"{"ok":false,"version":1,"applied":false,"guards":{"lid":false,"battery":false,"wifi":false,"display":false},"detail":"reply serialisation failed"}"#.to_string()
    })
}

/// Rolls the machine back if the UI stops checking in. Without this, an app crash
/// while protection is on would leave the laptop permanently unable to sleep — the
/// single worst failure this tool could have.
pub fn spawn_watchdog(helper: Arc<Mutex<Helper>>) {
    std::thread::spawn(move || loop {
        std::thread::sleep(WATCHDOG_TICK);
        if shutdown_requested() {
            return;
        }
        let mut h = helper.lock().unwrap();
        if h.applied && h.last_beat.elapsed().as_secs() > HEARTBEAT_TIMEOUT_SECS {
            eprintln!("[claude-awake] no heartbeat; restoring power settings");
            h.restore();
        }
    });
}

/// Called on every clean shutdown path. Leaving a machine unable to sleep because
/// the service stopped would be the worst possible parting gift.
pub fn restore_on_exit(helper: &Mutex<Helper>) {
    let mut h = helper.lock().unwrap();
    if h.applied {
        h.restore();
    }
}

// ── baseline persistence ────────────────────────────────────────────────────

fn baseline_path() -> &'static Path {
    Path::new(BASELINE_PATH)
}

fn read_baseline() -> Result<Option<Baseline>, String> {
    match fs::read_to_string(baseline_path()) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("could not read baseline: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read baseline: {e}")),
    }
}

fn load_or_capture_baseline() -> Result<Baseline, String> {
    // Reusing an existing snapshot matters: a second `Apply` must not record the
    // already-modified settings as the thing to return to.
    if let Some(existing) = read_baseline()? {
        return Ok(existing);
    }
    let baseline = power::capture_baseline()?;
    let path = baseline_path();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(&baseline).map_err(|e| e.to_string())?;
    fs::write(path, text).map_err(|e| format!("could not write baseline: {e}"))?;
    Ok(baseline)
}

fn clear_baseline() {
    let _ = fs::remove_file(baseline_path());
}

/// A baseline on disk at startup means we exited without restoring — power loss,
/// SIGKILL, reboot mid-session. Undo it before accepting any new work.
pub fn recover_from_unclean_exit() {
    match read_baseline() {
        Ok(Some(baseline)) => {
            eprintln!("[claude-awake] stale baseline found; restoring");
            if let Err(e) = power::restore(&baseline) {
                eprintln!("[claude-awake] restore failed: {e}");
            }
            clear_baseline();
        }
        Ok(None) => {}
        Err(e) => eprintln!("[claude-awake] baseline unreadable: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage_without_panicking() {
        let helper = Mutex::new(Helper::default());
        let reply: serde_json::Value =
            serde_json::from_str(&handle_line(&helper, "not json")).unwrap();
        assert_eq!(reply["ok"], false);
        assert!(reply["detail"].as_str().unwrap().contains("unrecognised"));
    }

    #[test]
    fn ping_reports_not_applied_before_any_apply() {
        let helper = Mutex::new(Helper::default());
        let reply: serde_json::Value =
            serde_json::from_str(&handle_line(&helper, r#"{"cmd":"ping"}"#)).unwrap();
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["applied"], false);
    }

    /// An unknown command must not be silently treated as something else — the
    /// helper is privileged, so "fail closed" is the only acceptable default.
    #[test]
    fn unknown_command_is_refused() {
        let helper = Mutex::new(Helper::default());
        let reply: serde_json::Value =
            serde_json::from_str(&handle_line(&helper, r#"{"cmd":"rm_rf"}"#)).unwrap();
        assert_eq!(reply["ok"], false);
    }
}
