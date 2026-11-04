use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::response::{parse_response, take_complete};
use super::{HttpResponse, CONNECT_TIMEOUT, MAX_RESPONSE_BYTES};

/// Issues one request and reads the complete response.
///
/// `target` must already be percent-encoded and start with `/`. Headers are
/// `(name, value)` pairs added after `Host`/`Connection`/`Content-Length`.
///
/// The whole socket exchange — connect, write *and* read — runs inside the
/// caller's `overall_timeout`. (An earlier version awaited `exchange` first
/// and only wrapped the already-finished value in `timeout`, so a controller
/// that accepted the socket but stopped answering hung a file operation
/// forever — the "no access to FluidNC files" symptom.)
pub(crate) async fn request(
    address: &str,
    method: &str,
    target: &str,
    headers: &[(&str, String)],
    body: Option<&[u8]>,
    overall_timeout: Duration,
) -> Result<HttpResponse, String> {
    let payload = build_request(address, method, target, headers, body);
    timeout(overall_timeout, exchange(address, method, &payload))
        .await
        .map_err(|_| format!("request timed out after {}s", overall_timeout.as_secs()))?
}

/// Performs connect → write → read. The read loop returns as soon as the
/// buffered bytes form a *complete* response (headers + declared body, or the
/// closing zero-size chunk); only a body of unknown length still waits for
/// the server to close the connection. FluidNC's AsyncWebServer honors
/// `Connection: close` on most builds, but a firmware that keeps the socket
/// open after `Content-Length` is now served instantly instead of freezing
/// the FILES panel until the overall timeout.
async fn exchange(address: &str, method: &str, payload: &[u8]) -> Result<HttpResponse, String> {
    let stream = TcpStream::connect(address)
        .await
        .map_err(|error| format!("connect {address}: {error}"))?;
    stream
        .set_nodelay(true)
        .map_err(|error| format!("nodelay {address}: {error}"))?;
    let (mut reader, mut writer) = stream.into_split();
    // The connect timeout only guards establishment; the caller's overall
    // timeout bounds the rest of the exchange.
    timeout(CONNECT_TIMEOUT, writer.write_all(payload))
        .await
        .map_err(|_| format!("{method} write timed out"))?
        .map_err(|error| format!("{method} write: {error}"))?;
    writer
        .flush()
        .await
        .map_err(|error| format!("{method} flush: {error}"))?;
    // NOTE: no `writer.shutdown()` here. The ESPAsyncWebServer stack on
    // FluidNC aborts an in-flight request when the client half-closes the
    // socket before the response has been produced: the FIN is treated as a
    // disconnect and the reply is dropped, which the client then reads as an
    // empty response ("missing header terminator" — the reported FluidNC
    // files bug). With `Connection: close` the server terminates the response
    // by itself; `take_complete` detects the finished body and the socket is
    // simply dropped when this function returns.

    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        // A self-delimiting body (Content-Length / closing chunk) finishes
        // the exchange without waiting for EOF.
        if let Some(response) =
            take_complete(&buffer).map_err(|error| format!("{method} response: {error}"))?
        {
            return Ok(response);
        }
        let read = reader
            .read(&mut chunk)
            .await
            .map_err(|error| format!("{method} read: {error}"))?;
        if read == 0 {
            // EOF: decode whatever arrived (unknown-length bodies end here).
            return parse_response(&buffer).map_err(|error| format!("{method} response: {error}"));
        }
        if buffer.len() + read > MAX_RESPONSE_BYTES {
            return Err(format!("response larger than {MAX_RESPONSE_BYTES} bytes"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

/// Serializes a plain-HTTP/1.1 request with `Connection: close`.
pub(super) fn build_request(
    address: &str,
    method: &str,
    target: &str,
    headers: &[(&str, String)],
    body: Option<&[u8]>,
) -> Vec<u8> {
    let mut text =
        format!("{method} {target} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
    if let Some(data) = body {
        text.push_str(&format!("Content-Length: {}\r\n", data.len()));
    }
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("\r\n");
    let mut payload = text.into_bytes();
    if let Some(data) = body {
        payload.extend_from_slice(data);
    }
    payload
}
