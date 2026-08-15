//! Client side of the helper connection. One connection per request keeps the
//! helper stateless with respect to sockets, so a UI crash can never wedge it.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use crate::protocol::{Reply, Request};

#[cfg(unix)]
const IO_TIMEOUT: Duration = Duration::from_secs(10);

pub fn request(req: &Request) -> Result<Reply, String> {
    let payload = serde_json::to_string(req).map_err(|e| e.to_string())?;
    let mut stream = connect()?;
    stream
        .write_all(format!("{payload}\n").as_bytes())
        .map_err(|e| format!("could not write to the helper: {e}"))?;
    stream.flush().ok();

    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|e| format!("the helper did not respond: {e}"))?;
    serde_json::from_str(&line).map_err(|e| format!("malformed reply: {e}"))
}

#[cfg(unix)]
fn connect() -> Result<std::os::unix::net::UnixStream, String> {
    use std::os::unix::net::UnixStream;
    let stream = UnixStream::connect(crate::protocol::SOCKET_PATH)
        .map_err(|e| format!("helper not running: {e}"))?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok();
    Ok(stream)
}

#[cfg(windows)]
fn connect() -> Result<std::fs::File, String> {
    /// All pipe instances busy. Transient — the helper is mid-handshake with
    /// another client, so retrying briefly is the documented response.
    const ERROR_PIPE_BUSY: i32 = 231;
    const ATTEMPTS: u32 = 10;

    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        // A byte-mode named pipe behaves like a file, which avoids pulling a
        // Windows-specific IPC crate into the UI process.
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(crate::protocol::PIPE_NAME)
        {
            Ok(file) => return Ok(file),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                last = e.to_string();
                std::thread::sleep(Duration::from_millis(50 * (attempt as u64 + 1)));
            }
            Err(e) => return Err(format!("helper not running: {e}")),
        }
    }
    Err(format!("helper busy: {last}"))
}
