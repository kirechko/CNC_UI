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

/// Issues one request and reads the complete response (until EOF).
///
/// `target` must already be percent-encoded and start with `/`. Headers are
/// `(name, value)` pairs added after `Host`/`Connection`/`Content-Length`.
pub(crate) async fn request(
    address: &str,
    method: &str,
    target: &str,
    headers: &[(&str, String)],
    body: Option<&[u8]>,
    overall_timeout: Duration,
) -> Result<HttpResponse, String> {
    let payload = build_request(address, method, target, headers, body);
    let task = exchange(address, method, &payload).await;
    timeout(overall_timeout, async { task })
        .await
        .map_err(|_| format!("request timed out after {}s", overall_timeout.as_secs()))?
}

/// Performs connect → write → read-to-EOF; separated so the blocking-style
/// socket work can be raced against the overall timeout in [`request`].
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
    // Half-close so a server waiting for more input terminates the response.
    let _ = writer.shutdown().await;

    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut chunk)
            .await
            .map_err(|error| format!("{method} read: {error}"))?;
        if read == 0 {
            break;
        }
        if buffer.len() + read > MAX_RESPONSE_BYTES {
            return Err(format!("response larger than {MAX_RESPONSE_BYTES} bytes"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    parse_response(&buffer).map_err(|error| format!("{method} response: {error}"))
}

/// Serializes a plain-HTTP/1.1 request with `Connection: close`.
fn build_request(
    address: &str,
    method: &str,
    target: &str,
    headers: &[(&str, String)],
    body: Option<&[u8]>,
) -> Vec<u8> {
    let mut text = format!("{method} {target} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
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
fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
    let header_end = find_header_end(raw).ok_or("missing header terminator")?;
    let head = std::str::from_utf8(&raw[..header_end])
        .map_err(|_| "non-UTF-8 response header")?
        .to_ascii_lowercase();
    let status = head
        .split('\n')
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.trim().parse::<u16>().ok())
        .ok_or("missing status line")?;
    let body_start = header_end + 4;
    let rest = &raw[body_start.min(raw.len())..];
    let chunked = head.lines().any(|line| {
        line.starts_with("transfer-encoding:") && line.contains("chunked")
    });
    let body = if chunked {
        decode_chunked(rest)?
    } else if let Some(length) = header_value(&head, "content-length").and_then(|v| v.parse::<usize>().ok()) {
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

/// Position of the `\r\n\r\n` header terminator, if any.
fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Lowercased value of one header from the already-lowercased header block.
fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find(|line| line.starts_with(&format!("{name}:")))
        .map(|line| line[name.len() + 1..].trim())
}

/// Decodes an HTTP chunked body: `hex-size [;ext]\r\n data \r\n` repeated
/// until the zero-size chunk. Trailer headers after the last chunk are
/// ignored (we never need them).
fn decode_chunked(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = find_line_end(data).ok_or("chunk size line missing")?;
        let size_text = std::str::from_utf8(&data[..line_end])
            .map_err(|_| "non-UTF-8 chunk size")?
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| format!("bad chunk size {size_text:?}"))?;
        if size == 0 {
            break;
        }
        if out.len() + size > MAX_RESPONSE_BYTES {
            return Err(format!("chunked body larger than {MAX_RESPONSE_BYTES} bytes"));
        }
        data = &data[line_end + 2..];
        if data.len() < size {
            return Err("chunk data truncated".to_string());
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        // Skip the CRLF after each chunk's payload.
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
    Ok(out)
}

/// Index of the next `\n` (the `\r` is stripped by the caller's trim).
fn find_line_end(data: &[u8]) -> Option<usize> {
    data.windows(2).position(|window| window == b"\r\n")
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
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3;ext=1\r\nabc\r\n0\r\n\r\n";
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
    fn parse_takes_whole_remainder_without_content_length() {
        // `Connection: close` means "read to EOF": an unknown-length body is
        // valid and must not be truncated at the first chunk-looking line.
        let response = parse_response(b"HTTP/1.1 200 OK\r\nX-Note: none\r\n\r\n4\r\nnope").expect("parse");
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
