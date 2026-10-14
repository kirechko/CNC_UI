 //! Serves newline-delimited JSON requests over the emulator's Unix socket.
use super::state::Emulator;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

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
fn write_response(
    stream: &mut UnixStream,
    result: Result<Value, WireError>,
) -> std::io::Result<()> {
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

pub(super) fn serve_client(stream: UnixStream, state: Arc<Mutex<Emulator>>) {
    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => {
                eprintln!("Socket read error: {e}");
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

        if let Err(error) = write_response(&mut writer, result) {
            eprintln!("Socket write error: {error}");
            return;
        }
    }
}
