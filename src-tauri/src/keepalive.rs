//! Unprivileged, session-level sleep prevention.
//!
//! This is the half that does *not* need root. It cannot stop a lid close on its
//! own — that is what the helper is for — but it covers idle sleep immediately and
//! keeps the machine up in a degraded but useful way when the helper is missing.

use std::sync::Mutex;

pub struct SessionKeepalive {
    inner: Mutex<Inner>,
}

impl Default for SessionKeepalive {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionKeepalive {
    pub fn new() -> Self {
        SessionKeepalive {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Idempotent: safe to call on every tick.
    pub fn set(&self, on: bool, keep_display: bool) {
        let mut inner = self.inner.lock().unwrap();
        inner.set(on, keep_display);
    }
}

#[cfg(target_os = "macos")]
mod imp {
    /// `caffeinate` is the supported front end to the IOKit assertions; shelling
    /// out to it avoids hand-rolling `IOPMAssertionCreateWithName` FFI and gets
    /// automatic cleanup: killing the child releases every assertion it holds.
    #[derive(Default)]
    pub struct Inner {
        child: Option<std::process::Child>,
        keep_display: bool,
    }

    impl Inner {
        pub fn set(&mut self, on: bool, keep_display: bool) {
            let already = self.child.is_some() && self.keep_display == keep_display;
            if on && already {
                return;
            }
            self.stop();
            if !on {
                return;
            }
            // -i idle, -m disk, -s system, -u user-active; -d also holds display.
            let flags = if keep_display { "-dimsu" } else { "-imsu" };
            // -w ties the child's lifetime to ours. Drop only runs on a graceful
            // exit; without this, a SIGKILL or a crash would orphan caffeinate and
            // leave the machine unable to sleep with nothing left to stop it.
            match std::process::Command::new("/usr/bin/caffeinate")
                .arg(flags)
                .arg("-w")
                .arg(std::process::id().to_string())
                .spawn()
            {
                Ok(child) => {
                    self.child = Some(child);
                    self.keep_display = keep_display;
                }
                Err(e) => eprintln!("[claude-awake] could not start caffeinate: {e}"),
            }
        }

        fn stop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use windows::Win32::System::Power::{
        SetThreadExecutionState, ES_AWAYMODE_REQUIRED, ES_CONTINUOUS, ES_DISPLAY_REQUIRED,
        ES_SYSTEM_REQUIRED, EXECUTION_STATE,
    };

    /// `ES_CONTINUOUS` is per-thread state that dies with the thread, so the flags
    /// live on a dedicated worker that outlives every call into it.
    pub struct Inner {
        want: Arc<AtomicU32>,
    }

    impl Default for Inner {
        fn default() -> Self {
            let want = Arc::new(AtomicU32::new(0));
            let worker = Arc::clone(&want);
            std::thread::spawn(move || {
                let mut current = u32::MAX;
                loop {
                    let desired = worker.load(Ordering::Relaxed);
                    if desired != current {
                        current = desired;
                        unsafe {
                            SetThreadExecutionState(EXECUTION_STATE(desired));
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            });
            Inner { want }
        }
    }

    impl Inner {
        pub fn set(&mut self, on: bool, keep_display: bool) {
            let flags = if !on {
                ES_CONTINUOUS
            } else if keep_display {
                ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_AWAYMODE_REQUIRED | ES_DISPLAY_REQUIRED
            } else {
                ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_AWAYMODE_REQUIRED
            };
            self.want.store(flags.0, Ordering::Relaxed);
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    #[derive(Default)]
    pub struct Inner;
    impl Inner {
        pub fn set(&mut self, _on: bool, _keep_display: bool) {}
    }
}

use imp::Inner;
