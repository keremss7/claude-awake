//! Finds the terminal window the overlay should stick to.
//!
//! The rule is deliberately strict: we only report a window when a terminal is the
//! *frontmost* app. Anchoring to a terminal that is buried behind other windows
//! would leave the pill floating over whatever the user is actually looking at.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as sys;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as sys;

/// Logical (point / DIP) coordinates, origin at the top-left of the primary
/// display — the same space Tauri's `LogicalPosition` uses.
#[derive(Clone, Debug, PartialEq)]
pub struct TermWindow {
    pub app: String,
    /// Owning process. Used to ignore our own windows — see `frontmost_window`.
    pub pid: u32,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Windows smaller than this are palettes, find bars or splash screens, not a
/// terminal worth anchoring to.
const MIN_W: f64 = 300.0;
const MIN_H: f64 = 140.0;

/// Owner names (macOS) matched case-insensitively against the frontmost window.
#[cfg(target_os = "macos")]
const TERMINALS: &[&str] = &[
    "terminal",
    "iterm2",
    "iterm",
    "ghostty",
    "wezterm",
    "wezterm-gui",
    "alacritty",
    "kitty",
    "warp",
    "warppreview",
    "hyper",
    "tabby",
    "rio",
    "code",
    "code - insiders",
    "vscodium",
    "cursor",
    "windsurf",
    "zed",
];

/// Executable basenames (Windows), lowercased, `.exe` included.
#[cfg(windows)]
const TERMINALS: &[&str] = &[
    "windowsterminal.exe",
    "wt.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "conhost.exe",
    "alacritty.exe",
    "wezterm-gui.exe",
    "ghostty.exe",
    "kitty.exe",
    "hyper.exe",
    "tabby.exe",
    "warp.exe",
    "code.exe",
    "cursor.exe",
    "windsurf.exe",
    "zed.exe",
];

pub fn is_terminal(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    TERMINALS.iter().any(|t| lower == *t)
}

/// Every on-screen window, front to back, paired with its window level. Exposed
/// because window tracking is the most brittle part of this app:
/// `examples/windows.rs` prints it so an unrecognised terminal can be identified
/// and added to `TERMINALS`, and so the overlay's own placement can be checked.
pub fn list_windows() -> Vec<(i64, TermWindow)> {
    sys::list_windows()
}

/// The frontmost *normal* window, whatever app owns it. Level 0 is the ordinary
/// application layer, so the first one in the list is what the user is looking at.
///
/// Our own windows are skipped. Otherwise opening the settings panel — a level-0
/// window like any other — would read as "a non-terminal is in front" and make the
/// pill vanish exactly while the user is configuring it.
pub fn frontmost_window() -> Option<TermWindow> {
    let own_pid = std::process::id();
    sys::list_windows()
        .into_iter()
        .find(|(layer, w)| *layer == 0 && w.pid != own_pid)
        .map(|(_, w)| w)
}

/// `None` means "no terminal in front" — the caller should hide the overlay.
pub fn frontmost_terminal() -> Option<TermWindow> {
    let w = frontmost_window()?;
    if !is_terminal(&w.app) || w.w < MIN_W || w.h < MIN_H {
        return None;
    }
    Some(w)
}

#[cfg(not(any(target_os = "macos", windows)))]
mod sys {
    pub fn list_windows() -> Vec<(i64, super::TermWindow)> {
        Vec::new()
    }
}
