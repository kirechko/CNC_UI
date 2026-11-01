//! Serves newline-delimited JSON requests over the emulator's TCP listener.
use super::state::Emulator;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::TcpStream;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub(super) struct Request {
    pub(super) method: String,
    #[serde(default)]
    pub(super) args: Value,
}

#[derive(Debug, Serialize)]
pub(super) struct WireError {
    pub(super) code: String,
    pub(super) message: String,
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    result: Value,
    error: Option<WireError>,
}

pub(super) fn api_error(code: impl Into<String>, message: impl Into<String>) -> WireError {
    WireError {
        code: code.into(),
        message: message.into(),
    }
}
fn write_response(stream: &mut TcpStream, result: Result<Value, WireError>) -> std::io::Result<()> {
    let response = match result {
        Ok(result) => Response {
            ok: true,
            result,
            error: None,
        },
        Err(error) => Response {
            ok: false,
            result: Value::Null,
            error: Some(error),
        },
    };
    serde_json::to_writer(&mut *stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

/// Processes newline-delimited requests until the client disconnects or shutdown is requested.
pub(super) fn serve_client(
    stream: TcpStream,
    state: Arc<Mutex<Emulator>>,
    should_stop: Arc<AtomicBool>,
) {
    let timeout = Some(Duration::from_millis(200));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;
    let mut line = String::new();

    loop {
        if should_stop.load(Ordering::Acquire) {
            return;
        }
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                continue;
            }
            Err(error) => {
                eprintln!("Socket read error: {error}");
                return;
            }
        }

        let parsed = serde_json::from_str::<Request>(&line)
            .map_err(|e| api_error("bad_json", e.to_string()));

        let result = match parsed {
            Ok(request) => match state.lock() {
                Ok(mut emulator) => emulator.handle(request),
                Err(_) => Err(api_error("internal", "state lock poisoned")),
            },
            Err(error) => Err(error),
        };
        if let Err(error) = &result {
            crate::message_log::record(
                "error",
                "emulator/protocol",
                &format!("{}: {}", error.code, error.message),
            );
        }

        if let Err(error) = write_response(&mut writer, result) {
            eprintln!("Socket write error: {error}");
            return;
        }
        line.clear();
    }
}
