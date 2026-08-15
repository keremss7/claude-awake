//! Shared, UI-free core. Nothing here may reference Tauri — the privileged helper
//! links this crate too, and it must stay a small, auditable binary.

pub mod detect;
pub mod helper;
pub mod ipc;
pub mod keepalive;
pub mod lidwatch;
pub mod power;
pub mod protocol;
pub mod tracker;
