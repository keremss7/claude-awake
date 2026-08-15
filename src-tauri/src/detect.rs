//! Is Claude Code doing anything worth staying awake for?
//!
//! Two tiers, because the honest answer depends on what the machine can see.
//!
//! **Precise (hooks installed).** Claude Code's hooks bracket real work exactly:
//! `UserPromptSubmit` opens a working turn, `Stop` closes it. Everything in
//! between — thinking, tool calls, subagents — is one continuous busy period, and
//! a session parked at an empty prompt is *not* busy. This is what lets protection
//! lift the moment the agent finishes, instead of holding a laptop awake all night
//! because a terminal happens to be open.
//!
//! **Coarse (no hooks).** Fall back to scanning for the `claude` process. It cannot
//! tell working from idle, so it errs toward staying awake. Correct, just wasteful.
//!
//! Hook state carries a TTL so a missed `Stop` or `SessionEnd` (crash, `kill -9`)
//! heals itself instead of pinning the machine awake forever.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Preferred loopback port; we walk forward if it is taken.
const BASE_PORT: u16 = 47821;
const PORT_TRIES: u16 = 12;
const POLL_INTERVAL: Duration = Duration::from_secs(4);
/// How long a hook-reported session stays trusted without any further signal.
/// A turn that runs longer than this without a single hook event is assumed dead.
const HOOK_TTL: Duration = Duration::from_secs(60 * 60);
/// Protection lingers this long after the last turn ends. Absorbs the gap between
/// a `Stop` and the follow-up prompt a user is already typing, so a conversation
/// does not flap the machine's power settings every few seconds.
const IDLE_GRACE: Duration = Duration::from_secs(90);
const MAX_BODY: usize = 64 * 1024;

pub struct Detector {
    proc_active: AtomicBool,
    hooks: Mutex<HookState>,
}

#[derive(Default)]
struct HookState {
    /// Session id -> when it last did something, and whether it is mid-turn.
    sessions: HashMap<String, SessionState>,
    /// When the last busy session went idle, for the grace period.
    last_busy_end: Option<Instant>,
    /// Set once any hook arrives. Until then the process scan is authoritative,
    /// because "no busy sessions" and "hooks are not installed" look identical.
    ever_reported: bool,
}

#[derive(Clone, Copy)]
struct SessionState {
    busy: bool,
    last_event: Instant,
}

impl Detector {
    pub fn start() -> Arc<Detector> {
        let detector = Arc::new(Detector {
            proc_active: AtomicBool::new(false),
            hooks: Mutex::new(HookState::default()),
        });

        {
            let d = Arc::clone(&detector);
            std::thread::spawn(move || loop {
                d.proc_active
                    .store(claude_process_running(), Ordering::Relaxed);
                std::thread::sleep(POLL_INTERVAL);
            });
        }
        {
            let d = Arc::clone(&detector);
            std::thread::spawn(move || d.serve());
        }
        detector
    }

    /// Should protection be engaged right now?
    pub fn active(&self) -> bool {
        let hooks = self.hooks.lock().unwrap();
        if hooks.ever_reported {
            // Hooks are authoritative once they have spoken: their whole value is
            // being able to say "a session is open but idle", which the process
            // scan can never do.
            return hooks.any_busy() || hooks.within_grace();
        }
        drop(hooks);
        self.proc_active.load(Ordering::Relaxed)
    }

    /// True when a Claude Code session exists at all, busy or not. Drives the UI
    /// hint rather than the power decision.
    pub fn session_present(&self) -> bool {
        if self.proc_active.load(Ordering::Relaxed) {
            return true;
        }
        let hooks = self.hooks.lock().unwrap();
        let present = hooks.live_sessions().next().is_some();
        present
    }

    /// True when a turn is actually in flight.
    pub fn busy(&self) -> bool {
        self.hooks.lock().unwrap().any_busy()
    }

    /// Whether hook events are driving the decision, as opposed to the coarse
    /// process scan. Surfaced in the settings panel so the difference is visible.
    pub fn precise(&self) -> bool {
        self.hooks.lock().unwrap().ever_reported
    }

    fn serve(self: Arc<Self>) {
        let Some(listener) = bind_loopback() else {
            eprintln!("[claude-awake] hook listener could not bind; process scan only");
            return;
        };
        if let Ok(addr) = listener.local_addr() {
            publish_endpoint(addr.port());
        }
        for stream in listener.incoming().flatten() {
            let me = Arc::clone(&self);
            std::thread::spawn(move || {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                me.handle(stream);
            });
        }
    }

    fn handle(&self, mut stream: TcpStream) {
        let Some((authorized, body)) = read_request(&mut stream) else {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
            return;
        };
        // Any web page can POST to loopback without CORS, so the shared token is
        // what stops a random site from pinning this laptop awake.
        if !authorized {
            let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
            return;
        }
        self.ingest(&body);
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    }

    /// Accepts a raw Claude Code hook payload.
    fn ingest(&self, body: &str) {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(body) else {
            return;
        };
        let event = json
            .get("hook_event_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let session = json
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let mut hooks = self.hooks.lock().unwrap();
        hooks.apply(&event, session);
    }
}

impl HookState {
    /// `UserPromptSubmit` opens a turn and `Stop` closes it — between them the
    /// agent is thinking or running tools, which is exactly the window worth
    /// staying awake for.
    fn apply(&mut self, event: &str, session: String) {
        self.ever_reported = true;
        let now = Instant::now();

        match event {
            "SessionEnd" => {
                if self.sessions.remove(&session).is_some_and(|s| s.busy) {
                    // A session killed mid-turn still ends the busy period.
                    self.note_idle(now);
                }
                return;
            }
            "Stop" | "SubagentStop" => {
                if let Some(state) = self.sessions.get_mut(&session) {
                    state.busy = false;
                    state.last_event = now;
                }
                self.note_idle(now);
                return;
            }
            _ => {}
        }

        // SessionStart registers an idle session. Anything else that arrives —
        // UserPromptSubmit, or a hook type that did not exist when this was
        // written — is evidence of work in flight, so it opens a turn. Erring
        // toward busy on an unknown event keeps a future Claude Code release from
        // silently letting the machine sleep mid-task.
        let busy = event != "SessionStart";
        self.sessions.insert(
            session,
            SessionState {
                busy,
                last_event: now,
            },
        );
    }

    fn note_idle(&mut self, now: Instant) {
        if !self.any_busy() {
            self.last_busy_end = Some(now);
        }
    }

    /// Sessions that have reported within the TTL. A session whose process died
    /// without firing `SessionEnd` ages out here rather than pinning the machine.
    fn live_sessions(&self) -> impl Iterator<Item = &SessionState> {
        self.sessions
            .values()
            .filter(|s| s.last_event.elapsed() < HOOK_TTL)
    }

    fn any_busy(&self) -> bool {
        self.live_sessions().any(|s| s.busy)
    }

    fn within_grace(&self) -> bool {
        self.last_busy_end.is_some_and(|t| t.elapsed() < IDLE_GRACE)
    }
}

fn bind_loopback() -> Option<TcpListener> {
    (0..PORT_TRIES)
        .find_map(|i| TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, BASE_PORT + i)).ok())
}

/// Returns `(token_matched, body)`. Deliberately minimal: this endpoint only ever
/// speaks to a `curl` line we generated ourselves.
fn read_request(stream: &mut TcpStream) -> Option<(bool, String)> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    if !line.starts_with("POST ") {
        return None;
    }

    let mut length = 0usize;
    let mut token = String::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0).min(MAX_BODY),
            "x-claude-awake" => token = value.trim().to_string(),
            _ => {}
        }
    }

    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some((
        constant_time_eq(token.as_bytes(), expected_token().as_bytes()),
        String::from_utf8_lossy(&body).into_owned(),
    ))
}

/// Directory holding the two files a shell hook needs to reach us.
pub fn endpoint_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(std::path::PathBuf::from(home).join(".claude-awake"))
}

/// Generated once, then persisted — a hook line in `settings.json` has to keep
/// working across app restarts, so the token cannot be per-process.
static TOKEN: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn expected_token() -> &'static str {
    TOKEN.get_or_init(|| {
        let path = endpoint_dir().map(|d| d.join("token"));
        if let Some(existing) = path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| s.len() == 32)
        {
            return existing;
        }
        let mut bytes = [0u8; 16];
        if getrandom::fill(&mut bytes).is_err() {
            // A predictable token is worse than no hooks at all: an empty token
            // never compares equal, so detection falls back to the process scan.
            return String::new();
        }
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        if let Some(path) = path {
            write_private(&path, &token);
        }
        token
    })
}

fn publish_endpoint(port: u16) {
    let Some(dir) = endpoint_dir() else { return };
    // Touch the token first so both files exist before any hook can fire.
    let _ = expected_token();
    write_private(&dir.join("port"), &port.to_string());
}

/// Writes owner-readable only — the token is what keeps a stray web page from
/// pinning this laptop awake.
fn write_private(path: &std::path::Path, contents: &str) {
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    if std::fs::write(path, contents).is_ok() {
        restrict_permissions(path);
    }
}

#[cfg(unix)]
fn restrict_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

/// On Windows the file inherits the ACL of `%USERPROFILE%\.claude-awake`, which
/// already excludes other users, so there is nothing to tighten.
#[cfg(not(unix))]
fn restrict_permissions(_path: &std::path::Path) {}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(unix)]
fn claude_process_running() -> bool {
    // `-x` is an exact match on the executable name, so this cannot be fooled by
    // an editor that happens to have "claude" in a file path.
    std::process::Command::new("/usr/bin/pgrep")
        .args(["-x", "claude"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false)
}

#[cfg(windows)]
fn claude_process_running() -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("tasklist.exe")
        .args(["/FI", "IMAGENAME eq claude.exe", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .to_lowercase()
                .contains("claude.exe")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tokens_never_match() {
        assert!(!constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"abc", b""));
    }

    #[test]
    fn equal_tokens_match() {
        assert!(constant_time_eq(b"deadbeef", b"deadbeef"));
        assert!(!constant_time_eq(b"deadbeef", b"deadbeee"));
    }

    fn state() -> HookState {
        HookState::default()
    }

    #[test]
    fn a_session_at_an_idle_prompt_is_not_worth_staying_awake_for() {
        let mut h = state();
        h.apply("SessionStart", "a".into());
        assert!(!h.any_busy(), "an open prompt is not work");
        assert!(!h.within_grace(), "nothing has run yet");
    }

    #[test]
    fn a_turn_brackets_the_busy_window() {
        let mut h = state();
        h.apply("SessionStart", "a".into());
        h.apply("UserPromptSubmit", "a".into());
        assert!(h.any_busy());
        h.apply("Stop", "a".into());
        assert!(!h.any_busy());
        // Still inside the linger, so protection has not dropped yet.
        assert!(h.within_grace());
    }

    #[test]
    fn one_busy_session_keeps_the_machine_up() {
        let mut h = state();
        h.apply("UserPromptSubmit", "a".into());
        h.apply("UserPromptSubmit", "b".into());
        h.apply("Stop", "a".into());
        assert!(h.any_busy(), "b is still working");
    }

    #[test]
    fn a_session_killed_mid_turn_stops_counting() {
        let mut h = state();
        h.apply("UserPromptSubmit", "a".into());
        h.apply("SessionEnd", "a".into());
        assert!(!h.any_busy());
    }

    /// A hook type that does not exist yet must not be read as "idle" — failing
    /// toward staying awake is the safe direction.
    #[test]
    fn unknown_events_are_treated_as_work() {
        let mut h = state();
        h.apply("SomeFutureHook", "a".into());
        assert!(h.any_busy());
    }

    #[test]
    fn hooks_only_become_authoritative_once_they_speak() {
        let mut h = state();
        assert!(
            !h.ever_reported,
            "process scan decides until a hook arrives"
        );
        h.apply("SessionStart", "a".into());
        assert!(h.ever_reported);
    }
}
