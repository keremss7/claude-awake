//! Wire format between the unprivileged UI app and the privileged helper.
//!
//! One JSON object per line, one request/response pair per connection. Keeping it
//! line-delimited means the helper never has to trust a length prefix from a
//! caller it does not control.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Seconds without a heartbeat before the helper assumes the UI died and puts the
/// machine's power settings back. Without this, a crashed app would leave a laptop
/// permanently unable to sleep.
pub const HEARTBEAT_TIMEOUT_SECS: u64 = 300;

#[cfg(unix)]
pub const SOCKET_PATH: &str = "/var/run/claude-awake.sock";

#[cfg(windows)]
pub const PIPE_NAME: &str = r"\\.\pipe\claude-awake";

/// Where the helper stores the power settings it found before touching anything.
/// Its presence at helper startup means we exited uncleanly and must roll back.
#[cfg(unix)]
pub const BASELINE_PATH: &str = "/Library/Application Support/ClaudeAwake/baseline.json";

#[cfg(windows)]
pub const BASELINE_PATH: &str = r"C:\ProgramData\ClaudeAwake\baseline.json";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    /// Also prevent the display from sleeping. Off by default — with the lid shut
    /// there is nothing to look at, and holding the panel on costs battery.
    pub keep_display: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Liveness + version check.
    Ping,
    /// Apply the sleep-prevention profile, snapshotting current settings first.
    Apply(Profile),
    /// Roll back to the snapshot taken by the first `Apply`.
    Restore,
    /// Keeps the watchdog from rolling back under a healthy UI.
    Heartbeat,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Guards {
    /// The lid can be closed without the machine sleeping.
    pub lid: bool,
    /// Battery level and low-power transitions are ignored.
    pub battery: bool,
    /// The Wi-Fi radio stays powered and network wake is armed.
    pub wifi: bool,
    /// The display is additionally held on.
    pub display: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reply {
    pub ok: bool,
    pub version: u32,
    pub applied: bool,
    pub guards: Guards,
    /// Human-readable note — surfaced verbatim in the settings panel when
    /// something went wrong, so failures are never silent.
    pub detail: String,
}

impl Reply {
    pub fn err(detail: impl Into<String>) -> Self {
        Reply {
            ok: false,
            version: PROTOCOL_VERSION,
            applied: false,
            guards: Guards::default(),
            detail: detail.into(),
        }
    }
}
