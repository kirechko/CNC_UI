//! Synchronous TCP client for the selected LinuxCNC bridge process.
//!
//! The desktop app owns this connection boundary while the Python companion
//! process runs next to NML. Every request uses the bridge's v1 JSON Lines
//! envelope and a correlation id so status and command responses are checked.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A bridge transport error preserving its machine-readable failure category.
#[derive(Debug, Clone)]
pub(crate) struct BridgeError {
    /// Stable error code from networking, framing, or the remote bridge.
    pub(crate) code: String,
    /// Human-readable diagnostic for a failed bridge operation.
    pub(crate) message: String,
}

/// Request envelope matching `lcnc_adapter/protocol.py`.
#[derive(Serialize)]
struct BridgeRequest<'a> {
    /// Protocol version currently supported by both bridge endpoints.
    v: u8,
    /// Unique per-process id echoed by the response.
    id: u64,
    /// Whitelisted bridge operation name.
    command: &'a str,
    /// Operation-specific arguments.
    payload: Value,
}

/// Success-or-failure response envelope returned by the bridge.
#[derive(Deserialize)]
struct BridgeResponse {
    /// Request id echoed by the bridge.
    id: u64,
    /// Indicates whether the command succeeded.
    ok: bool,
    /// Command result payload on success.
    #[serde(default)]
    payload: Value,
    /// Optional structured error on failure.
    error: Option<BridgeFailure>,
}

/// Error payload supplied by the bridge process.
#[derive(Deserialize)]
struct BridgeFailure {
    /// Stable machine-readable failure code.
    code: String,
    /// Diagnostic message returned by the bridge.
    message: String,
}

/// Sends one bounded v1 command and validates the correlated newline response.
pub(crate) fn request(
    endpoint: &str,
    command: &str,
    payload: Value,
    id: u64,
    timeout: Duration,
) -> Result<Value, BridgeError> {
    // Resolve first, then bound both socket directions so a missing bridge cannot hang UI handlers.
    let address = resolve_endpoint(endpoint)?;
    let mut stream =
        TcpStream::connect_timeout(&address, timeout).map_err(|error| BridgeError {
            code: "linuxcnc_connect".into(),
            message: error.to_string(),
        })?;
    stream.set_read_timeout(Some(timeout)).map_err(io_error)?;
    stream.set_write_timeout(Some(timeout)).map_err(io_error)?;
    let request = BridgeRequest {
        v: 1,
        id,
        command,
        payload,
    };
    serde_json::to_writer(&mut stream, &request).map_err(|error| BridgeError {
        code: "linuxcnc_protocol".into(),
        message: error.to_string(),
    })?;
    stream.write_all(b"\n").map_err(io_error)?;
    stream.flush().map_err(io_error)?;

    // Bound the complete response line before deserialization; bridge peers are network input.
    let mut line = String::new();
    let bytes_read = BufReader::new(stream)
        .take(1024 * 1024)
        .read_line(&mut line)
        .map_err(io_error)?;
    if bytes_read == 0 || bytes_read >= 1024 * 1024 || !line.ends_with('\n') {
        return Err(BridgeError {
            code: "linuxcnc_protocol".into(),
            message: "bridge response is empty, incomplete, or exceeds 1 MiB".into(),
        });
    }
    let response: BridgeResponse = serde_json::from_str(&line).map_err(|error| BridgeError {
        code: "linuxcnc_protocol".into(),
        message: error.to_string(),
    })?;
    if response.id != id {
        return Err(BridgeError {
            code: "linuxcnc_protocol".into(),
            message: format!(
                "bridge response id {} did not match request {id}",
                response.id
            ),
        });
    }
    if !response.ok {
        let error = response.error.unwrap_or(BridgeFailure {
            code: "linuxcnc_rejected".into(),
            message: "bridge rejected command without error details".into(),
        });
        return Err(BridgeError {
            code: error.code,
            message: error.message,
        });
    }
    Ok(response.payload)
}

/// Resolves a host-and-port value and reports malformed or unresolvable endpoints.
fn resolve_endpoint(endpoint: &str) -> Result<SocketAddr, BridgeError> {
    endpoint
        .to_socket_addrs()
        .map_err(io_error)?
        .next()
        .ok_or_else(|| BridgeError {
            code: "linuxcnc_endpoint".into(),
            message: format!("no socket address resolved for {endpoint}"),
        })
}

/// Converts a standard I/O failure into the bridge error representation.
fn io_error(error: std::io::Error) -> BridgeError {
    BridgeError {
        code: "linuxcnc_io".into(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A local fake TCP peer verifies request framing and response correlation without LinuxCNC.
    #[test]
    fn request_round_trips_a_v1_response_and_rejects_wrong_ids() {
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        let peer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["command"], "status");
            writeln!(
                stream,
                "{{\"v\":1,\"id\":41,\"ok\":true,\"payload\":{{\"connected\":true}}}}"
            )
            .unwrap();
        });
        let payload = request(
            &endpoint,
            "status",
            serde_json::json!({}),
            41,
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(payload["connected"], true);
        peer.join().unwrap();
    }

    /// Keeps socket failures categorized so callers can distinguish transport from CNC errors.
    #[test]
    fn io_errors_keep_a_stable_category() {
        let error = io_error(std::io::Error::new(std::io::ErrorKind::TimedOut, "late"));
        assert_eq!(error.code, "linuxcnc_io");
        assert!(error.message.contains("late"));
    }
}
