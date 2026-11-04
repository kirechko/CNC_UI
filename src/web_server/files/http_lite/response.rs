use super::{HttpResponse, MAX_RESPONSE_BYTES};

/// Decodes a *complete* response from the bytes received so far.
///
/// `Ok(None)` means "more bytes needed" (headers still arriving, declared
/// body truncated, chunk stream unterminated, or an unknown-length body that
/// only ends at EOF). Malformed framing is a hard `Err`, never a wait — a
/// garbage status line will never fix itself with more bytes.
pub(super) fn take_complete(raw: &[u8]) -> Result<Option<HttpResponse>, String> {
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

/// Splits one complete response buffer into status, headers and decoded body.
///
/// Task: FluidNC file access failed with "GET response: missing header
/// terminator". espressif's HTTP stack does not always emit the RFC-mandated
/// CRLF pairs: some builds terminate the header block (and chunk lines) with
/// bare `\n`. The parser therefore accepts BOTH `\r\n\r\n` and `\n\n` as the
/// header terminator and treats bare-LF chunk lines like CRLF ones.
pub(super) fn parse_response(raw: &[u8]) -> Result<HttpResponse, String> {
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
