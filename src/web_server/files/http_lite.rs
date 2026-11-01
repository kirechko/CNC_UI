//! A minimal HTTP/1.1 client over `tokio::net::TcpStream`.
//!
//! The project has no HTTP client dependency (no reqwest), and the FluidNC
//! controller only needs plain-HTTP requests on one LAN address: send a
//! request, read the response until the peer closes the connection
//! (`Connection: close`). Chunked transfer encoding is decoded because
//! espressiff `AsyncWebServer` may chunk large bodies; gzip is never
//! requested since we do not send `Accept-Encoding`.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// TCP connect timeout: FluidNC is on the LAN, anything slower than this is
/// effectively unreachable and the UI should learn it quickly.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// Hard ceiling for one response body; protects the backend from a runaway
/// server while still allowing full SD-card files.
const MAX_RESPONSE_BYTES: usize = 128 * 1024 * 1024;

/// A fully received HTTP response.
pub(crate) struct HttpResponse {
    /// Numeric status code (`201`, `404`, `507`, …).
    pub(crate) status: u16,
    /// Response body after transfer-decoding.
    pub(crate) body: Vec<u8>,
}

/// Builds the request target for a percent-encoded path plus query string.
///
/// Keeps the `/` separators readable for the server and escapes everything
/// that is not unreserved per RFC 3986, matching what FluidNC's URL parser
/// expects (`%20` spaces, `%C3%BC` UTF-8 names, etc.).
pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

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

/// Decodes a *complete* response from the bytes received so far.
///
/// `Ok(None)` means "more bytes needed" (headers still arriving, declared
/// body truncated, chunk stream unterminated, or an unknown-length body that
/// only ends at EOF). Malformed framing is a hard `Err`, never a wait — a
/// garbage status line will never fix itself with more bytes.
fn take_complete(raw: &[u8]) -> Result<Option<HttpResponse>, String> {
    let Some((header_end, terminator_len)) = find_header_end(raw) else {
        // Header block still incomplete; guard against a runaway server.
        if raw.len() > MAX_RESPONSE_BYTES {
            return Err(format!("response larger than {MAX_RESPONSE_BYTES} bytes"));
        }
        return Ok(None);
    };
    let head = std::str::from_utf8(&raw[..header_end])
        .map_err(|_| "non-UTF-8 response header")?
        .to_ascii_lowercase();
    if head
        .split('\n')
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.trim().parse::<u16>().ok())
        .is_none()
    {
        return Err("missing status line".to_string());
    }
    let rest = &raw[(header_end + terminator_len).min(raw.len())..];
    let chunked = head
        .lines()
        .any(|line| line.starts_with("transfer-encoding:") && line.contains("chunked"));
    if chunked {
        // Complete once the terminating zero-size chunk line is present; the
        // trailers after it are ignored, so no further wait is needed.
        return if chunked_terminator_seen(rest)? {
            Ok(Some(parse_response(raw)?))
        } else {
            Ok(None)
        };
    }
    if let Some(length) =
        header_value(&head, "content-length").and_then(|v| v.parse::<usize>().ok())
    {
        return Ok((rest.len() >= length)
            .then(|| parse_response(raw))
            .transpose()?);
    }
    // No length information at all: the body ends only at EOF, so the read
    // loop keeps collecting until the peer closes the socket.
    Ok(None)
}

/// Walks the chunk framing looking for the zero-size terminator without
/// decoding payloads; `Ok(false)` while the stream is still incomplete.
fn chunked_terminator_seen(data: &[u8]) -> Result<bool, String> {
    let mut cursor = data;
    loop {
        let (line_end, line_terminator_len) = match find_line_end(cursor) {
            Some(found) => found,
            // Size line still arriving.
            None => return Ok(false),
        };
        let size_text = std::str::from_utf8(&cursor[..line_end])
            .map_err(|_| "non-UTF-8 chunk size")?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("bad chunk size {size_text:?}"))?;
        if size == 0 {
            return Ok(true);
        }
        let payload_start = (line_end + line_terminator_len).min(cursor.len());
        cursor = &cursor[payload_start..];
        if cursor.len() < size {
            return Ok(false);
        }
        cursor = &cursor[size..];
        // Skip the CRLF/LF after the chunk payload when present; if absent
        // the frame line is incomplete and the next walk simply waits.
        if cursor.starts_with(b"\r\n") {
            cursor = &cursor[2..];
        } else if cursor.starts_with(b"\n") {
            cursor = &cursor[1..];
        }
    }
}

/// Serializes a plain-HTTP/1.1 request with `Connection: close`.
fn build_request(
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

/// Splits one complete response buffer into status, headers and decoded body.
///
/// Task: FluidNC file access failed with "GET response: missing header
/// terminator". espressif's HTTP stack does not always emit the RFC-mandated
/// CRLF pairs: some builds terminate the header block (and chunk lines) with
/// bare `\n`. The parser therefore accepts BOTH `\r\n\r\n` and `\n\n` as the
/// header terminator and treats bare-LF chunk lines like CRLF ones.
fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let (header_end, terminator_len) = find_header_end(raw).ok_or("missing header terminator")?;
    let head = std::str::from_utf8(&raw[..header_end])
        .map_err(|_| "non-UTF-8 response header")?
        .to_ascii_lowercase();
    let status = head
        .split('\n')
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.trim().parse::<u16>().ok())
        .ok_or("missing status line")?;
    let body_start = (header_end + terminator_len).min(raw.len());
    let rest = &raw[body_start..];
    let chunked = head
        .lines()
        .any(|line| line.starts_with("transfer-encoding:") && line.contains("chunked"));
    let body = if chunked {
        decode_chunked(rest)?
    } else if let Some(length) =
        header_value(&head, "content-length").and_then(|v| v.parse::<usize>().ok())
    {
        // With `Connection: close` we normally read to EOF; trust EOF but
        // surface a short body so truncating middle-proxy bugs are visible.
        if rest.len() < length {
            return Err(format!("body truncated: {} of {length} bytes", rest.len()));
        }
        rest[..length].to_vec()
    } else {
        rest.to_vec()
    };
    Ok(HttpResponse { status, body })
}

/// Position and length of the header terminator: `\r\n\r\n` (2+2 bytes) or,
/// for servers that emit bare LF, `\n\n` (1+1 bytes). The earliest match of
/// either form wins; a mixed `\r\n\n` / `\n\r\n` is also covered because the
/// scan looks at the raw byte windows directly.
fn find_header_end(raw: &[u8]) -> Option<(usize, usize)> {
    let crlf = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| (index, 4));
    let lf = raw
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|index| (index, 2));
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Lowercased value of one header from the already-lowercased header block.
fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find(|line| line.starts_with(&format!("{name}:")))
        .map(|line| line[name.len() + 1..].trim())
}

/// Decodes an HTTP chunked body: `hex-size [;ext]<line-end> data <line-end>`
/// repeated until the zero-size chunk. Line endings may be CRLF or bare LF —
/// both are produced by different espressif AsyncWebServer revisions. Trailer
/// headers after the last chunk are ignored (we never need them).
fn decode_chunked(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let (line_end, line_terminator_len) =
            find_line_end(data).ok_or("chunk size line missing")?;
        let size_text = std::str::from_utf8(&data[..line_end])
            .map_err(|_| "non-UTF-8 chunk size")?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("bad chunk size {size_text:?}"))?;
        if size == 0 {
            break;
        }
        if out.len() + size > MAX_RESPONSE_BYTES {
            return Err(format!(
                "chunked body larger than {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        data = &data[(line_end + line_terminator_len).min(data.len())..];
        if data.len() < size {
            return Err("chunk data truncated".to_string());
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        // Skip the CRLF/LF after each chunk's payload.
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        } else if data.starts_with(b"\n") {
            data = &data[1..];
        }
    }
    Ok(out)
}

/// Index and length of the next line terminator: `\r\n` (length 2) or a bare
/// `\n` (length 1), whichever appears first.
fn find_line_end(data: &[u8]) -> Option<(usize, usize)> {
    let crlf = data.windows(2).position(|window| window == b"\r\n");
    let lf = data.iter().position(|byte| *byte == b'\n');
    match (crlf, lf) {
        (Some(c), Some(l)) => {
            if l == c + 1 {
                Some((c, 2))
            } else if c < l {
                Some((c, 2))
            } else {
                Some((l, 1))
            }
        }
        (Some(c), None) => Some((c, 2)),
        (None, Some(l)) => Some((l, 1)),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encode_keeps_structure_and_escapes_the_rest() {
        assert_eq!(percent_encode("/flash/dir"), "/flash/dir");
        assert_eq!(percent_encode("a b~c.d_e-f"), "a%20b~c.d_e-f");
        // UTF-8 multi-byte letters become their raw byte escapes.
        assert_eq!(percent_encode("über.nc"), "%C3%BCber.nc");
        assert_eq!(percent_encode("100%#"), "100%25%23");
    }

    #[test]
    fn take_complete_waits_for_headers_and_body() {
        // Half a header block: keep reading.
        assert!(take_complete(b"HTTP/1.1 200 OK\r\nCont").unwrap().is_none());
        // Headers + short of the declared body: still incomplete.
        assert!(
            take_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhel")
                .unwrap()
                .is_none()
        );
        // Exactly complete: answered without waiting for the peer's EOF.
        let done = take_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
            .unwrap()
            .expect("complete");
        assert_eq!(done.status, 200);
        assert_eq!(done.body, b"hello");
        // Unknown length (no Content-Length, not chunked): body ends at EOF.
        assert!(
            take_complete(b"HTTP/1.1 200 OK\r\nX-Note: none\r\n\r\npartial")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn take_complete_detects_the_chunked_terminator() {
        assert!(take_complete(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n"
        )
        .unwrap()
        .is_none());
        assert!(take_complete(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n0\r\n\r\n"
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn parse_plain_response_with_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        let response = parse_response(raw).expect("parse");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"hello");
    }

    #[test]
    fn parse_response_without_content_length_reads_to_end() {
        let raw = b"HTTP/1.1 204 No Content\r\nAllow: GET\r\n\r\n";
        let response = parse_response(raw).expect("parse");
        assert_eq!(response.status, 204);
        assert!(response.body.is_empty());
    }

    #[test]
    fn decode_chunked_strips_sizes_and_trailer() {
        let raw =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
        let response = parse_response(raw).expect("parse");
        assert_eq!(response.body, b"Wikipedia");
    }

    #[test]
    fn decode_chunked_handles_size_extensions() {
        let raw =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;ext=1\r\nabc\r\n0\r\n\r\n";
        let response = parse_response(raw).expect("parse");
        assert_eq!(response.body, b"abc");
    }

    #[test]
    fn parse_rejects_truncated_body_and_garbage() {
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nabc").is_err());
        assert!(parse_response(b"garbage").is_err());
        assert!(parse_response(b"HTTP/1.1").is_err());
    }

    #[test]
    fn parse_accepts_bare_lf_header_terminator() {
        // espressif builds that emit LF-only line endings must not fail with
        // "missing header terminator" (the reported FluidNC file error).
        let raw = b"HTTP/1.1 200 OK\nContent-Length: 5\n\nhello";
        let response = parse_response(raw).expect("parse LF headers");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"hello");
    }

    #[test]
    fn decode_chunked_accepts_bare_lf_lines() {
        let raw = b"HTTP/1.1 200 OK\nTransfer-Encoding: chunked\n\n4\nWiki\n5\npedia\n0\n\n";
        let response = parse_response(raw).expect("parse LF chunked");
        assert_eq!(response.body, b"Wikipedia");
    }

    #[test]
    fn parse_takes_whole_remainder_without_content_length() {
        // `Connection: close` means "read to EOF": an unknown-length body is
        // valid and must not be truncated at the first chunk-looking line.
        let response =
            parse_response(b"HTTP/1.1 200 OK\r\nX-Note: none\r\n\r\n4\r\nnope").expect("parse");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"4\r\nnope");
    }

    #[test]
    fn build_request_includes_headers_and_body() {
        let payload = build_request(
            "192.168.1.10:80",
            "PUT",
            "/flash/a%20b",
            &[("Destination", "http://x/y".to_string())],
            Some(b"data"),
        );
        let text = String::from_utf8(payload).expect("utf8");
        assert!(text.starts_with("PUT /flash/a%20b HTTP/1.1\r\n"));
        assert!(text.contains("Host: 192.168.1.10:80\r\n"));
        assert!(text.contains("Content-Length: 4\r\n"));
        assert!(text.contains("Destination: http://x/y\r\n"));
        assert!(text.ends_with("\r\n\r\ndata"));
    }
}
