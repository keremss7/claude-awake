//! Diagnostic: what does Claude Awake think is in front right now?
//!
//!     cargo run --example windows          # follow the frontmost window
//!     cargo run --example windows -- --all # dump every on-screen window once
//!
//! If your terminal shows up as "not a terminal", add the printed owner name to
//! `TERMINALS` in src/tracker/mod.rs. The `--all` dump also shows the overlay
//! itself (a raised window level), which is how you verify it is parked where you
//! expect.

use std::time::Duration;

use claude_awake_lib::tracker;

fn main() {
    if std::env::args().any(|a| a == "--all") {
        println!("{:<24}{:>6}  {:<20}", "owner", "level", "bounds");
        for (level, w) in tracker::list_windows() {
            println!(
                "{:<24}{:>6}  {:.0},{:.0} {:.0}x{:.0}",
                w.app, level, w.x, w.y, w.w, w.h
            );
        }
        return;
    }

    println!("Watching the frontmost window — Ctrl-C to stop.\n");
    let mut last = String::new();

    loop {
        let line = match tracker::frontmost_window() {
            Some(w) => format!(
                "{:<22} {:>5.0},{:<5.0} {:>5.0}x{:<5.0}  {}",
                w.app,
                w.x,
                w.y,
                w.w,
                w.h,
                if tracker::is_terminal(&w.app) {
                    "terminal — the pill shows here"
                } else {
                    "not a terminal — the pill hides"
                }
            ),
            None => "(no frontmost window)".to_string(),
        };
        if line != last {
            println!("{line}");
            last = line;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}
