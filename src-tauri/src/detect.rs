//! Is a Claude Code session running?
//!
//! Two independent signals, OR-ed together:
//!
//!   * **Process scan** — the source of truth. `claude` is a native binary, so an
//!     exact-name match is unambiguous. Costs one `pgrep` every few seconds.
//!   * **Hook pings** — Claude Code's `SessionStart` / `SessionEnd` hooks POST to a
//!     loopback port. This removes the poll latency and covers sessions the process
//!     scan cannot see. Hook state carries a TTL so a missed `SessionEnd` (crash,
//!     `kill -9`) heals itself instead of pinning the machine awake forever.

use std::collections::HashSet;
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
const HOOK_TTL: Duration = Duration::from_secs(15 * 60);
const MAX_BODY: usize = 64 * 1024;

pub struct Detector {
    proc_active: AtomicBool,
    hooks: Mutex<HookState>,
}

#[derive(Default)]
struct HookState {
    sessions: HashSet<String>,
    last_seen: Option<Instant>,
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

    pub fn active(&self) -> bool {
        if self.proc_active.load(Ordering::Relaxed) {
            return true;
        }
        let hooks = self.hooks.lock().unwrap();
        !hooks.sessions.is_empty() && hooks.last_seen.is_some_and(|t| t.elapsed() < HOOK_TTL)
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
        match event.as_str() {
            "SessionEnd" => {
                hooks.sessions.remove(&session);
            }
            // Anything else that arrives is evidence the session is alive; treating
            // unknown events as keepalives means new hook types cannot break this.
            _ => {
                hooks.sessions.insert(session);
            }
        }
        hooks.last_seen = Some(Instant::now());
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
}
