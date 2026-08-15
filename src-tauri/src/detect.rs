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
/// How long a session is remembered at all without any hook event.
const HOOK_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// How long a turn stays believed-busy on the strength of its last event.
///
/// A missed `Stop` (crash, kill -9, a hook that failed to fire) would otherwise
/// pin the machine awake forever, so being busy is a claim that has to be
/// renewed. Tool-call hooks renew it constantly; the window only has to be wider
/// than the longest realistic gap between them, which is a stretch of pure
/// reasoning with no tool use.
const ACTIVITY_TTL: Duration = Duration::from_secs(20 * 60);
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

/// One row in the settings panel's session list.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    /// Human-readable: the working directory's basename, or a short id.
    pub label: String,
    pub busy: bool,
    pub ignored: bool,
    pub idle_secs: u64,
}

impl SessionState {
    /// Whether this session should hold the machine awake.
    fn counts(&self) -> bool {
        self.busy && !self.ignored
    }

    fn label(&self, id: &str) -> String {
        self.cwd
            .as_deref()
            .and_then(|p| p.rsplit(['/', '\\']).next())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| id.chars().take(8).collect())
    }
}

#[derive(Clone)]
struct SessionState {
    busy: bool,
    last_event: Instant,
    /// Working directory from the hook payload. A UUID is useless in a list of
    /// four sessions; "monorepo" tells you which one is holding the machine up.
    cwd: Option<String>,
    /// Excluded from the awake decision by the user. A background session that
    /// churns all day should not be able to pin the laptop on its own.
    ignored: bool,
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

    /// Every live session, newest activity first. The awake decision is
    /// machine-wide — one busy session keeps everything up — so listing them is
    /// what makes "why is this still awake" answerable at a glance.
    pub fn sessions(&self) -> Vec<SessionInfo> {
        let hooks = self.hooks.lock().unwrap();
        let mut list: Vec<SessionInfo> = hooks
            .sessions
            .iter()
            .filter(|(_, s)| s.last_event.elapsed() < HOOK_TTL)
            .map(|(id, s)| SessionInfo {
                id: id.clone(),
                label: s.label(id),
                busy: s.busy && s.last_event.elapsed() < ACTIVITY_TTL,
                ignored: s.ignored,
                idle_secs: s.last_event.elapsed().as_secs(),
            })
            .collect();
        list.sort_by_key(|s| s.idle_secs);
        list
    }

    /// Excludes a session from the awake decision, or puts it back.
    pub fn set_session_ignored(&self, id: &str, ignored: bool) {
        let mut hooks = self.hooks.lock().unwrap();
        if let Some(state) = hooks.sessions.get_mut(id) {
            state.ignored = ignored;
        }
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
        let Some(request) = read_request(&mut stream) else {
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
            return;
        };
        // Any web page can POST to loopback without CORS, so the shared token is
        // what stops a random site from pinning this laptop awake.
        if !request.authorized {
            let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
            return;
        }
        if request.diagnostic {
            let body = self.describe();
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                )
                .as_bytes(),
            );
            return;
        }
        self.ingest(&request.body);
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
    }

    /// Dumps what the detector currently believes. Exists because "why did it
    /// think Claude was idle" is otherwise unanswerable after the fact — the whole
    /// decision lives in memory. Token-gated like everything else on this port.
    ///
    /// ```text
    /// curl -sS -H "X-Claude-Awake: $(cat ~/.claude-awake/token)" \
    ///      -X POST --data diagnose "http://127.0.0.1:$(cat ~/.claude-awake/port)/state"
    /// ```
    fn describe(&self) -> String {
        let hooks = self.hooks.lock().unwrap();
        let sessions: Vec<serde_json::Value> = hooks
            .sessions
            .iter()
            .map(|(id, state)| {
                serde_json::json!({
                    "session": id,
                    "label": state.label(id),
                    "busy": state.busy,
                    "ignored": state.ignored,
                    "secsSinceEvent": state.last_event.elapsed().as_secs(),
                    "claimFresh": state.last_event.elapsed() < ACTIVITY_TTL,
                })
            })
            .collect();
        serde_json::json!({
            "hooksReporting": hooks.ever_reported,
            "processScanSeesClaude": self.proc_active.load(Ordering::Relaxed),
            "anyBusy": hooks.any_busy(),
            "withinGrace": hooks.within_grace(),
            "secsSinceLastTurnEnded": hooks.last_busy_end.map(|t| t.elapsed().as_secs()),
            "sessions": sessions,
        })
        .to_string()
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
        let cwd = json
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let mut hooks = self.hooks.lock().unwrap();
        hooks.apply(&event, session, cwd);
    }
}

impl HookState {
    /// `UserPromptSubmit` opens a turn and `Stop` closes it — between them the
    /// agent is thinking or running tools, which is exactly the window worth
    /// staying awake for.
    fn apply(&mut self, event: &str, session: String, cwd: Option<String>) {
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
            // Only the main agent's Stop ends a turn. SubagentStop fires while
            // the parent is still working, so treating it as the end would drop
            // protection in the middle of exactly the long fan-out runs this tool
            // exists for.
            "Stop" => {
                if let Some(state) = self.sessions.get_mut(&session) {
                    state.busy = false;
                    state.last_event = now;
                }
                self.note_idle(now);
                return;
            }
            _ => {}
        }

        // SessionStart registers an idle session. Everything else — a submitted
        // prompt, a tool call, a subagent finishing, or a hook type that did not
        // exist when this was written — is evidence of work in flight. Erring
        // toward busy on an unknown event keeps a future Claude Code release from
        // silently letting the machine sleep mid-task.
        let busy = event != "SessionStart";
        let entry = self.sessions.entry(session).or_insert(SessionState {
            busy,
            last_event: now,
            cwd: cwd.clone(),
            ignored: false,
        });
        entry.busy = busy;
        entry.last_event = now;
        if cwd.is_some() {
            entry.cwd = cwd;
        }
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

    /// A session counts as busy only while its claim is fresh. Without the age
    /// check a single missed `Stop` would hold the machine up indefinitely.
    fn any_busy(&self) -> bool {
        self.sessions
            .values()
            .any(|s| s.counts() && s.last_event.elapsed() < ACTIVITY_TTL)
    }

    fn within_grace(&self) -> bool {
        self.last_busy_end.is_some_and(|t| t.elapsed() < IDLE_GRACE)
    }
}

fn bind_loopback() -> Option<TcpListener> {
    (0..PORT_TRIES)
        .find_map(|i| TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, BASE_PORT + i)).ok())
}

struct HookRequest {
    authorized: bool,
    /// True for `/state`, which reports rather than mutates.
    diagnostic: bool,
    body: String,
}

/// Deliberately minimal: this endpoint only ever speaks to a `curl` line we
/// generated ourselves.
fn read_request(stream: &mut TcpStream) -> Option<HookRequest> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    if !line.starts_with("POST ") {
        return None;
    }
    let diagnostic = line.split_whitespace().nth(1) == Some("/state");

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
    Some(HookRequest {
        authorized: constant_time_eq(token.as_bytes(), expected_token().as_bytes()),
        diagnostic,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
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

    /// Test shorthand; the cwd only matters for the display label.
    fn send(h: &mut HookState, event: &str, session: &str) {
        h.apply(event, session.to_string(), None);
    }

    #[test]
    fn a_session_at_an_idle_prompt_is_not_worth_staying_awake_for() {
        let mut h = state();
        send(&mut h, "SessionStart", "a");
        assert!(!h.any_busy(), "an open prompt is not work");
        assert!(!h.within_grace(), "nothing has run yet");
    }

    #[test]
    fn a_turn_brackets_the_busy_window() {
        let mut h = state();
        send(&mut h, "SessionStart", "a");
        send(&mut h, "UserPromptSubmit", "a");
        assert!(h.any_busy());
        send(&mut h, "Stop", "a");
        assert!(!h.any_busy());
        // Still inside the linger, so protection has not dropped yet.
        assert!(h.within_grace());
    }

    #[test]
    fn one_busy_session_keeps_the_machine_up() {
        let mut h = state();
        send(&mut h, "UserPromptSubmit", "a");
        send(&mut h, "UserPromptSubmit", "b");
        send(&mut h, "Stop", "a");
        assert!(h.any_busy(), "b is still working");
    }

    /// The reported failure: a long turn emits no `Stop`, so without tool-call
    /// heartbeats it ages out of the activity window and protection drops
    /// mid-work.
    #[test]
    fn tool_activity_renews_a_long_running_turn() {
        let mut h = state();
        send(&mut h, "UserPromptSubmit", "a");
        // Simulate the claim going stale, as it would during a multi-hour job.
        h.sessions.get_mut("a").unwrap().last_event = Instant::now() - ACTIVITY_TTL * 2;
        assert!(!h.any_busy(), "a stale claim must not hold the machine up");

        send(&mut h, "PostToolUse", "a");
        assert!(h.any_busy(), "a tool call proves the turn is still alive");
    }

    /// A subagent finishing says nothing about the parent turn, which is usually
    /// still fanning out more work.
    #[test]
    fn subagent_stop_does_not_end_the_turn() {
        let mut h = state();
        send(&mut h, "UserPromptSubmit", "a");
        send(&mut h, "SubagentStop", "a");
        assert!(h.any_busy());
    }

    /// A `Stop` that never arrives must not pin the machine awake forever.
    #[test]
    fn a_missed_stop_heals_itself() {
        let mut h = state();
        send(&mut h, "UserPromptSubmit", "a");
        h.sessions.get_mut("a").unwrap().last_event = Instant::now() - ACTIVITY_TTL * 2;
        assert!(!h.any_busy());
    }

    #[test]
    fn a_session_killed_mid_turn_stops_counting() {
        let mut h = state();
        send(&mut h, "UserPromptSubmit", "a");
        send(&mut h, "SessionEnd", "a");
        assert!(!h.any_busy());
    }

    /// A hook type that does not exist yet must not be read as "idle" — failing
    /// toward staying awake is the safe direction.
    #[test]
    fn unknown_events_are_treated_as_work() {
        let mut h = state();
        send(&mut h, "SomeFutureHook", "a");
        assert!(h.any_busy());
    }

    #[test]
    fn hooks_only_become_authoritative_once_they_speak() {
        let mut h = state();
        assert!(
            !h.ever_reported,
            "process scan decides until a hook arrives"
        );
        send(&mut h, "SessionStart", "a");
        assert!(h.ever_reported);
    }
}
