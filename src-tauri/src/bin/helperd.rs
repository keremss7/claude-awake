//! `claude-awake-helperd` — the privileged half of Claude Awake.
//!
//! Deliberately a separate binary from the UI: the thing running as root/SYSTEM
//! links no window system, no webview and no HTTP client, which keeps it small
//! enough to audit in one sitting. All of its logic lives in
//! `claude_awake_lib::helper`.

fn main() {
    claude_awake_lib::helper::run();
}
