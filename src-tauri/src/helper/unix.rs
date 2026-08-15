//! Unix transport: a filesystem socket at `/var/run/claude-awake.sock`.
//!
//! Permissions are 0660 root:staff. Every interactive macOS account is in `staff`,
//! so a normal login session can reach the helper without being root — while a
//! sandboxed or service account cannot.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{handle_line, Helper};
use crate::protocol::SOCKET_PATH;

/// macOS `staff`. Members are exactly the human accounts on the machine.
const STAFF_GID: u32 = 20;

const READ_TIMEOUT: Duration = Duration::from_secs(15);

pub fn run() {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("claude-awake-helperd must run as root");
        std::process::exit(1);
    }

    super::recover_from_unclean_exit();

    let path = Path::new(SOCKET_PATH);
    // A leftover socket file from a hard kill would make bind() fail forever.
    let _ = std::fs::remove_file(path);
    let listener = match UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("could not bind {SOCKET_PATH}: {e}");
            std::process::exit(1);
        }
    };
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660));
    unsafe {
        if let Ok(c_path) = std::ffi::CString::new(SOCKET_PATH) {
            libc::chown(c_path.as_ptr(), 0, STAFF_GID);
        }
    }

    let helper = Arc::new(Mutex::new(Helper::default()));
    super::spawn_watchdog(Arc::clone(&helper));
    install_signal_handlers();
    eprintln!("[claude-awake] helper listening on {SOCKET_PATH}");

    for stream in listener.incoming().flatten() {
        if super::shutdown_requested() {
            break;
        }
        let helper = Arc::clone(&helper);
        // A thread per request keeps one slow or wedged client from blocking the
        // next; requests are tiny and short-lived.
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
            let Ok(mut writer) = stream.try_clone() else {
                return;
            };
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_err() {
                return;
            }
            let reply = handle_line(&helper, &line);
            let _ = writer.write_all(format!("{reply}\n").as_bytes());
        });
    }

    super::restore_on_exit(&helper);
    let _ = std::fs::remove_file(path);
}

/// launchd sends SIGTERM on `bootout`. Restoring there means a clean uninstall
/// never depends on the watchdog timing out.
fn install_signal_handlers() {
    unsafe {
        let handler = handle_signal as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }

    // `accept()` blocks with no timeout, so something has to nudge it. That nudge
    // is a socket connection, which allocates — not async-signal-safe, and so not
    // something the handler itself may do. This thread does it from normal
    // context once it sees the flag.
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if super::shutdown_requested() {
            let _ = std::os::unix::net::UnixStream::connect(SOCKET_PATH);
            return;
        }
    });
}

/// Async-signal-safe: a single atomic store and nothing else.
extern "C" fn handle_signal(_sig: libc::c_int) {
    super::request_shutdown();
}
