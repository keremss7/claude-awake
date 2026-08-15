//! Platform power control. Everything in here runs inside the privileged helper —
//! nothing in this module may assume a UI, a user session, or a $HOME.

use crate::protocol::{Guards, Profile};

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos as sys;

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use windows as sys;

/// Opaque, platform-specific record of the power settings as they were before we
/// touched anything. Stored as JSON so the helper can roll back even across a
/// restart or a crash.
pub type Baseline = serde_json::Value;

pub struct ApplyOutcome {
    pub guards: Guards,
    /// Non-fatal problems (a key this hardware does not support, say). Surfaced to
    /// the user rather than swallowed, so a half-applied profile is visible.
    pub warnings: Vec<String>,
}

/// Read the current settings. Called once, before the first `apply`.
pub fn capture_baseline() -> Result<Baseline, String> {
    sys::capture_baseline()
}

/// Push the sleep-prevention profile. Must be idempotent: the UI re-applies on
/// every profile change and after helper restarts.
pub fn apply(baseline: &Baseline, profile: Profile) -> Result<ApplyOutcome, String> {
    sys::apply(baseline, profile)
}

/// Put everything back exactly as `capture_baseline` found it.
pub fn restore(baseline: &Baseline) -> Result<(), String> {
    sys::restore(baseline)
}
