//! Turns the internal display off when the lid closes.
//!
//! Normally macOS does this itself: the lid switch powers the panel down on the
//! way into clamshell sleep. `disablesleep 1` suppresses that whole path, so with
//! protection on the panel stays lit behind a closed lid until the idle timer
//! eventually expires — burning battery and trapping heat in a bag.
//!
//! So when the lid closes and the user has *not* asked to keep the display on, we
//! put it to sleep explicitly. `pmset displaysleepnow` needs no privileges and
//! only affects the display; system sleep stays blocked.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Lid state is cheap to read but not free — it shells out to `ioreg`. Two
/// seconds is well inside the window where a lit panel matters.
const POLL: Duration = Duration::from_secs(2);

pub struct LidWatcher {
    /// True while protection is on *and* the user has not opted to keep the
    /// display awake.
    armed: Arc<AtomicBool>,
}

impl Default for LidWatcher {
    fn default() -> Self {
        Self::start()
    }
}

impl LidWatcher {
    pub fn start() -> LidWatcher {
        let armed = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "macos")]
        {
            let armed = Arc::clone(&armed);
            std::thread::spawn(move || watch(armed));
        }
        LidWatcher { armed }
    }

    /// Idempotent; safe to call on every tick.
    pub fn set_armed(&self, armed: bool) {
        self.armed.store(armed, Ordering::Relaxed);
    }
}

#[cfg(target_os = "macos")]
fn watch(armed: Arc<AtomicBool>) {
    let mut was_closed = false;
    loop {
        std::thread::sleep(POLL);
        let Some(closed) = lid_closed() else { continue };

        // Only act on the transition. Repeating `displaysleepnow` every tick would
        // fight the user if they wake the display deliberately with an external
        // keyboard while the lid is shut.
        if closed && !was_closed && armed.load(Ordering::Relaxed) {
            let _ = std::process::Command::new("/usr/bin/pmset")
                .arg("displaysleepnow")
                .status();
        }
        was_closed = closed;
    }
}

/// `None` when the state cannot be read — a desktop Mac has no clamshell key.
#[cfg(target_os = "macos")]
fn lid_closed() -> Option<bool> {
    let out = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-r", "-k", "AppleClamshellState", "-d", "4"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .find(|l| l.contains("\"AppleClamshellState\""))?;
    Some(line.contains("Yes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arming_is_idempotent() {
        let watcher = LidWatcher::start();
        watcher.set_armed(true);
        watcher.set_armed(true);
        assert!(watcher.armed.load(Ordering::Relaxed));
        watcher.set_armed(false);
        assert!(!watcher.armed.load(Ordering::Relaxed));
    }
}
