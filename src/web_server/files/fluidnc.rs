//! FluidNC controller file systems over plain HTTP.
//!
//! Task: the controller's files must be reachable over HTTP from the FILES
//! panel. Three official interfaces are combined (per the upstream
//! `WebServer.cpp` / `Webdav.cpp` firmware sources), consulted in order of
//! decreasing stability so one flaky or missing endpoint never breaks the
//! panel:
//!
//! 1. **Action API listing** — `GET /files?...` for the flash volume and
//!    `GET /upload?...` for the SD card. The JSON footer carries volume
//!    usage; directory entries come as `size == -1` or with explicit
//!    `dir`/`isDir`/`isFile` flags depending on the firmware revision.
//!    Both the legacy `action=list&filename=all` and the newer
//!    `command=list&format=json` spellings are tried.
//! 2. **WebDAV `PROPFIND` (Depth: 1)** on `/flash` / `/sd` — deterministic
//!    XML listing that works even on builds whose action API changed.
//! 3. **WebDAV mutations** for everything else, because WebDAV status codes
//!    are deterministic while the action API returns human-readable text:
//!    `MKCOL` (201/405), `DELETE` (204/404/413), `MOVE` (2xx, same volume
//!    only), `PUT` (201/507) and `GET` (200/404/503). `COPY` is advertised
//!    in `Allow` but never dispatched, so cross-volume copies are performed
//!    by the dispatcher as download + upload.
//!
//! The HTTP port is always the controller's web port (80 by default); a
//! Telnet-discovered device still exposes the same server, so the endpoint
//! port from discovery is only trusted for the WebSocket transport.
//!
//! A FluidNC *controlled* over a COM port (task: serial FluidNC + HTTP files)
//! has no web server behind the cable itself, but the serial scan registers
//! the controller's Wi-Fi twin whenever its banner exposes an address; the
//! address lookup resolves through that twin, so file exchange still runs
//! over HTTP while machine control stays on the serial link.

use std::time::Duration;

use serde_json::Value;

use super::super::state::AppState;
use super::{http_lite, join_path, normalize_path, FileEntry, FileListResult, FsError, FsSource};

/// Timeout for metadata-sized requests (listing, mkdir, delete, move).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Timeout for bodies (file download and upload over a possibly busy WiFi).
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(120);

/// WebDAV mount prefix of one source volume.
fn volume_prefix(source: FsSource) -> &'static str {
    match source {
        FsSource::Controller => "/flash",
        FsSource::Sd => "/sd",
        FsSource::Local => "/",
    }
}

/// Action-API endpoint that lists one source volume.
fn action_endpoint(source: FsSource) -> &'static str {
    match source {
        FsSource::Controller => "/files",
        FsSource::Sd => "/upload",
        FsSource::Local => "/files",
    }
}

/// Derives `host:port` of the controller HTTP server from a discovered
/// machine id (`fluidnc:<Transport>:<endpoint>`). Serial and socket transports
/// have no FluidNC HTTP server and yield `None`.
fn parse_machine_id(id: &str) -> Option<String> {
    let tail = id.strip_prefix("fluidnc:")?;
    let (transport, endpoint) = tail.split_once(':')?;
    match transport {
        "WebSocket" => {
            // Endpoint already carries the discovered port (80 by default).
            Some(if endpoint.contains(':') {
                endpoint.to_string()
            } else {
                format!("{endpoint}:80")
            })
        }
        "Telnet" => {
            let host = endpoint.rsplit(':').next_back()?;
            Some(format!("{host}:80"))
        }
        _ => None,
    }
}

/// HTTP address of the currently connected FluidNC controller.
///
/// Network transports resolve their address from the machine id. A controller
/// connected over a COM port resolves through its Wi-Fi twin — the
/// `fluidnc:WebSocket:…` row the serial scanner registered under the same
/// machine name — so the FILES panel keeps working while jog/program traffic
/// stays on the cable (task: HTTP file exchange for serial FluidNC). Without a
/// twin (Wi-Fi disabled) there is genuinely no HTTP server to talk to and the
/// dedicated error keeps the controller/sd source chips unavailable.
pub(crate) fn device_address(state: &AppState) -> Result<String, FsError> {
    let id = state
        .connected_machine_id()
        .ok_or_else(|| FsError::new("files.error.notConnected", ""))?;
    if !id.starts_with("fluidnc:") {
        return Err(FsError::new("files.error.notFluidNc", ""));
    }
    if let Some(address) = parse_machine_id(&id) {
        return Ok(address);
    }
    // Serial connection: match the network twin by machine name (the banner
    // `[MSG:Machine:…]` value both discovery rows carry).
    let snapshot = state.machine_snapshot();
    let name = snapshot
        .iter()
        .find(|device| device.id == id)
        .and_then(|device| device.machine_name.clone());
    if let Some(name) = name {
        let twin = snapshot.iter().find(|device| {
            device.protocol == "FluidNC"
                && device.transport == cnc_core::discovery::DiscoveryTransport::WebSocket
                && device.machine_name.as_deref() == Some(name.as_str())
        });
        if let Some(address) = twin.and_then(|device| parse_machine_id(&device.id)) {
            return Ok(address);
        }
    }
    Err(FsError::new("files.error.serialNoHttp", id))
}

/// Wraps transport failures into the `deviceHttp` i18n key.
fn transport_error(detail: String) -> FsError {
    FsError::new("files.error.deviceHttp", detail)
}

/// One numeric-or-string JSON quantity. FluidNC firmware 4.x prints the
/// volume footer as *human-readable* strings (`"total":"1.94 MB"`,
/// `"used":"168.00 KB"`, `"occupation":"8"`) while older builds send bare
/// numbers, so both shapes are accepted; `human_bytes` interprets the unit
/// suffix (binary KiB/MiB convention of the firmware's own formatter).
fn bytes_value(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => parse_size_string(text.trim()),
        _ => None,
    }
}

/// Parses `"2401"`, `"168.00 KB"`, `"1.94 MB"`, `"3 GB"` into bytes.
/// A pure integer keeps its value; anything with a unit suffix multiplies
/// with the binary step (KB=1024), matching how the firmware formats `used`.
fn parse_size_string(text: &str) -> Option<u64> {
    if let Ok(plain) = text.parse::<u64>() {
        return Some(plain);
    }
    let (number, unit) = text
        .split_once(char::is_whitespace)
        .map(|(value, rest)| (value, rest.trim()))
        .unwrap_or((text, ""));
    let value: f64 = number.trim().parse().ok()?;
    let scale: f64 = match unit.to_ascii_uppercase().as_str() {
        "B" | "" => 1.0,
        "KB" | "KIB" | "K" => 1024.0,
        "MB" | "MIB" | "M" => 1024.0 * 1024.0,
        "GB" | "GIB" | "G" => 1024.0 * 1024.0 * 1024.0,
        "TB" | "TIB" | "T" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = value * scale;
    if bytes.is_finite() && bytes >= 0.0 && bytes <= u64::MAX as f64 {
        Some(bytes.round() as u64)
    } else {
        None
    }
}

/// Parses one action-API listing payload into a [`FileListResult`].
fn parse_list_json(raw: &[u8], source: FsSource, dir: &str) -> Result<FileListResult, FsError> {
    let parsed: Value = serde_json::from_slice(raw)
        .map_err(|error| transport_error(format!("listing is not JSON: {error}")))?;
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("");
    let Some(files) = parsed.get("files").and_then(Value::as_array) else {
        // Firmware answers `{"status":"No SD card"}` when the SD mount fails.
        if status.eq_ignore_ascii_case("No SD card") || source == FsSource::Sd {
            return Err(FsError::new("files.error.noSdCard", status));
        }
        return Err(transport_error(format!("listing failed: {status}")));
    };
    let mut entries = Vec::with_capacity(files.len());
    for item in files {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        // FluidNC reports -1 for directories on the classic action API; some
        // newer firmware revisions instead carry an explicit `dir`/`isDir`/
        // `isFile` boolean, so all three conventions are honored. The `size`
        // value itself may arrive as a JSON number or as a decimal *string*
        // (firmware 4.x prints `"size":"2401"`), so both are parsed.
        let raw_size = item
            .get("size")
            .and_then(|value| match value {
                Value::Number(number) => number.as_i64(),
                Value::String(text) => text.trim().parse::<i64>().ok(),
                _ => None,
            })
            .unwrap_or(0);
        let is_dir = raw_size < 0
            || item.get("dir").and_then(Value::as_bool).unwrap_or(false)
            || item.get("isDir").and_then(Value::as_bool).unwrap_or(false)
            || item.get("isFile").and_then(Value::as_bool) == Some(false);
        entries.push(FileEntry {
            name: name.to_string(),
            path: join_path(dir, name),
            is_dir,
            size: if is_dir { 0 } else { raw_size.max(0) as u64 },
            modified_ms: None,
        });
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(FileListResult {
        path: dir.to_string(),
        entries,
        total_bytes: bytes_value(parsed.get("total")),
        used_bytes: bytes_value(parsed.get("used")),
        // `occupation` is a number on old builds and a string (`"8"`) on 4.x.
        occupation_percent: parsed
            .get("occupation")
            .and_then(|value| bytes_value(Some(value)))
            .map(|value| value.min(u8::MAX as u64) as u8),
    })
}

/// Validates a normalized manager path for use on a FluidNC volume.
fn remote_path(path: &str) -> Result<String, FsError> {
    normalize_path(path).ok_or_else(|| FsError::new("files.error.badPath", path))
}

/// One action-API GET decoded into a listing, or a [`FsError`] describing the
/// first failure the caller should surface if no fallback succeeds either.
async fn action_list(
    address: &str,
    source: FsSource,
    dir: &str,
    query: &str,
) -> Result<FileListResult, FsError> {
    let target = format!("{}?{query}", action_endpoint(source));
    let response = http_lite::request(address, "GET", &target, &[], None, REQUEST_TIMEOUT)
        .await
        .map_err(transport_error)?;
    if response.status != 200 {
        return Err(transport_error(format!(
            "listing HTTP status {}",
            response.status
        )));
    }
    parse_list_json(&response.body, source, dir)
}

/// Lists one directory on a FluidNC volume, preferring the action API and
/// falling back to WebDAV `PROPFIND` (task: HTTP file access must work across
/// firmware revisions — the classic `action=list` spelling, the newer
/// `command=list&format=json` and the WebDAV mount are tried in order until
/// one returns a directory listing).
pub(crate) async fn list(
    address: &str,
    source: FsSource,
    path: &str,
) -> Result<FileListResult, FsError> {
    let dir = remote_path(path)?;
    let encoded = http_lite::percent_encode(&dir);
    let attempts = [
        format!("action=list&filename=all&path={encoded}"),
        format!("command=list&format=json&path={encoded}"),
    ];
    // Keep the first meaningful error: a missing SD card is a real answer,
    // not a transport glitch, and must not be masked by the fallbacks below.
    let mut first_error: Option<FsError> = None;
    for query in attempts {
        match action_list(address, source, &dir, &query).await {
            Ok(list) => return Ok(list),
            Err(error) if error.key == "files.error.noSdCard" => return Err(error),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    // PROPFIND over the WebDAV mount: works on builds whose action API moved
    // or returns non-JSON footers.
    match propfind_list(address, source, &dir).await {
        Ok(list) => Ok(list),
        Err(propfind_error) => {
            let detail = format!(
                "{} (PROPFIND fallback: {})",
                first_error
                    .map(|error| error.detail)
                    .unwrap_or_else(|| "no listing".to_string()),
                propfind_error.detail
            );
            Err(FsError::new("files.error.deviceHttp", detail))
        }
    }
}

/// Requests one WebDAV directory with `PROPFIND Depth: 1` and decodes the
/// multistatus XML. FluidNC's `Webdav.cpp` emits a `<d:response>` block per
/// entry; a `<d:collection/>` node marks directories. Only the flat subset
/// the firmware produces is parsed (no href-decomposed paths are assumed
/// beyond the last path segment).
async fn propfind_list(
    address: &str,
    source: FsSource,
    dir: &str,
) -> Result<FileListResult, FsError> {
    let target = format!(
        "{}{}",
        volume_prefix(source),
        http_lite::percent_encode(dir)
    );
    let response = http_lite::request(
        address,
        "PROPFIND",
        &target,
        &[
            ("Depth", "1".to_string()),
            ("Accept", "application/xml".to_string()),
        ],
        // Minimal WebDAV body: an empty `propfind` requesting allprops, which
        // AsyncWebServer's PROPFIND handler accepts even though the schema
        // technically expects `<allprop/>` (firmware ignores the body today).
        Some(br#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:allprop/></d:propfind>"#),
        REQUEST_TIMEOUT,
    )
    .await
    .map_err(transport_error)?;
    // 207 Multi-Status is the normative answer; some builds return 200.
    if response.status != 207 && response.status != 200 {
        return Err(transport_error(format!(
            "PROPFIND {dir} -> HTTP {}",
            response.status
        )));
    }
    let xml = String::from_utf8_lossy(&response.body).into_owned();
    Ok(parse_multistatus(&xml, dir))
}

/// Parses the `<d:response>` items of a PROPFIND multistatus payload.
///
/// Tag discovery is case-insensitive (firmware revisions emit `D:`, `d:` or
/// unprefixed tags), but every extracted text is sliced from the ORIGINAL
/// body: `to_ascii_lowercase` preserves byte length, so offsets computed on
/// the lowercase copy map 1:1 onto the source. Blindly lowercasing the whole
/// payload would corrupt the file names inside `<href>` and break the
/// follow-up WebDAV GET/PUT that uses those paths.
fn parse_multistatus(xml: &str, dir: &str) -> FileListResult {
    let lowered = xml.to_ascii_lowercase();
    // The requested directory itself also appears as a `<response>` item and
    // must not be listed among its children.
    let self_name = dir.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
    let mut entries = Vec::new();
    let mut cursor = 0_usize;
    while let Some((offset, opening)) = next_response_opening(&lowered[cursor..]) {
        let block_start = cursor + offset + opening.len();
        let closing = format!("</{}>", &opening[1..]);
        let Some(end) = lowered[block_start..].find(&closing) else {
            // Unterminated final block: nothing else can be parsed reliably.
            break;
        };
        let raw_block = &xml[block_start..block_start + end];
        let lowered_block = &lowered[block_start..block_start + end];
        if let Some(entry) = multistatus_entry(raw_block, lowered_block, dir, &self_name) {
            entries.push(entry);
        }
        cursor = block_start + end + closing.len();
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    FileListResult {
        path: dir.to_string(),
        entries,
        total_bytes: None,
        used_bytes: None,
        occupation_percent: None,
    }
}

/// Earliest `<d:response`/`<response>` opening tag inside `text`, returned as
/// `(offset, matched token)`; the token drives the matching closing tag.
fn next_response_opening(text: &str) -> Option<(usize, &'static str)> {
    let mut best: Option<(usize, &'static str)> = None;
    for tag in ["<d:response", "<response"] {
        if let Some(at) = text.find(tag) {
            if best.is_none_or(|(offset, _)| at < offset) {
                best = Some((at, tag));
            }
        }
    }
    best
}

/// Extracts one entry from a `<response>` block. `None` when the block is the
/// requested directory itself or carries no readable href.
fn multistatus_entry(raw: &str, lowered: &str, dir: &str, self_name: &str) -> Option<FileEntry> {
    let href = tag_content(lowered, raw, "href")?.trim().to_string();
    let decoded = percent_decode(&href);
    let name = decoded.rsplit('/').next().unwrap_or("").to_string();
    if name.is_empty() || name.to_ascii_lowercase() == self_name {
        return None;
    }
    let is_dir = lowered.contains("<collection") || href.ends_with('/');
    let size = tag_content(lowered, raw, "getcontentlength")
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0);
    Some(FileEntry {
        name,
        path: join_path(dir, &decoded.rsplit('/').next().unwrap_or("").to_string()),
        is_dir,
        size: if is_dir { 0 } else { size },
        modified_ms: None,
    })
}

/// Text between `<name …>` and `</name>`, located case-insensitively in
/// `lowered` while the returned slice comes from `raw` at identical offsets
/// (ASCII lowercasing is length-preserving). A content tag never nested in a
/// closing tag thanks to the `<name` prefix plus delimiter check.
fn tag_content<'a>(lowered: &str, raw: &'a str, name: &str) -> Option<&'a str> {
    let pattern = format!("<{name}");
    let closing = format!("</{name}>");
    let bytes = lowered.as_bytes();
    let mut search = 0_usize;
    while let Some(found) = lowered[search..].find(&pattern) {
        let open = search + found;
        let after = open + pattern.len();
        // Accept only a real tag boundary (`>`/`/`/whitespace), so a
        // hypothetical `<hrefs>` never matches.
        if !matches!(
            bytes.get(after),
            Some(b'>') | Some(b'/') | Some(b' ') | Some(b'\t')
        ) {
            search = after;
            continue;
        }
        let gt = lowered[after..].find('>')?;
        let content_start = after + gt + 1;
        let end = lowered[content_start..].find(&closing)?;
        return raw.get(content_start..content_start + end);
    }
    None
}

/// Percent-decodes `%XX` sequences; everything else passes through. A strict
/// RFC 3986 decoder is unnecessary: the firmware only escapes path bytes, and
/// a malformed escape must not lose the whole listing.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&text[index + 1..index + 3], 16) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Downloads one whole file through WebDAV `GET`.
pub(crate) async fn read_bytes(
    address: &str,
    source: FsSource,
    path: &str,
) -> Result<Vec<u8>, FsError> {
    let target = format!(
        "{}{}",
        volume_prefix(source),
        http_lite::percent_encode(&remote_path(path)?)
    );
    let response = http_lite::request(address, "GET", &target, &[], None, TRANSFER_TIMEOUT)
        .await
        .map_err(transport_error)?;
    match response.status {
        200..=299 => Ok(response.body),
        404 => Err(FsError::new("files.error.notFound", path)),
        503 => Err(FsError::new("files.error.volumeUnavailable", path)),
        status => Err(transport_error(format!("GET {path} -> HTTP {status}"))),
    }
}

/// Uploads one whole file through WebDAV `PUT` (overwrite is allowed).
pub(crate) async fn write_bytes(
    address: &str,
    source: FsSource,
    path: &str,
    data: &[u8],
) -> Result<(), FsError> {
    let target = format!(
        "{}{}",
        volume_prefix(source),
        http_lite::percent_encode(&remote_path(path)?)
    );
    let response = http_lite::request(
        address,
        "PUT",
        &target,
        &[("Content-Type", "application/octet-stream".to_string())],
        Some(data),
        TRANSFER_TIMEOUT,
    )
    .await
    .map_err(transport_error)?;
    match response.status {
        200..=299 => Ok(()),
        507 => Err(FsError::new("files.error.volumeFull", path)),
        403 => Err(FsError::new("files.error.denied", path)),
        404 => Err(FsError::new("files.error.notFound", path)),
        status => Err(transport_error(format!("PUT {path} -> HTTP {status}"))),
    }
}

/// Creates one directory through WebDAV `MKCOL`.
pub(crate) async fn make_dir(address: &str, source: FsSource, path: &str) -> Result<(), FsError> {
    let target = format!(
        "{}{}",
        volume_prefix(source),
        http_lite::percent_encode(&remote_path(path)?)
    );
    let response = http_lite::request(address, "MKCOL", &target, &[], None, REQUEST_TIMEOUT)
        .await
        .map_err(transport_error)?;
    match response.status {
        200..=299 => Ok(()),
        405 => Err(FsError::new("files.error.alreadyExists", path)),
        507 => Err(FsError::new("files.error.volumeFull", path)),
        status => Err(transport_error(format!("MKCOL {path} -> HTTP {status}"))),
    }
}

/// Deletes one file or directory through WebDAV `DELETE`.
pub(crate) async fn remove(address: &str, source: FsSource, path: &str) -> Result<(), FsError> {
    let target = format!(
        "{}{}",
        volume_prefix(source),
        http_lite::percent_encode(&remote_path(path)?)
    );
    let response = http_lite::request(address, "DELETE", &target, &[], None, REQUEST_TIMEOUT)
        .await
        .map_err(transport_error)?;
    match response.status {
        200..=299 => Ok(()),
        404 => Err(FsError::new("files.error.notFound", path)),
        // FluidNC replies 413 when a directory is not empty or removal fails.
        413 => Err(FsError::new("files.error.io", "directory not empty")),
        status => Err(transport_error(format!("DELETE {path} -> HTTP {status}"))),
    }
}

/// Moves/renames within one volume through WebDAV `MOVE`; the destination is
/// a full URL whose path keeps the same mount prefix.
pub(crate) async fn move_path(
    address: &str,
    source: FsSource,
    path: &str,
    to_dir: &str,
    new_name: &str,
) -> Result<(), FsError> {
    if new_name.is_empty() || new_name.contains('/') {
        return Err(FsError::new("files.error.badPath", new_name));
    }
    let from = remote_path(path)?;
    let to = join_path(&remote_path(to_dir)?, new_name);
    let prefix = volume_prefix(source);
    let target = format!("{prefix}{}", http_lite::percent_encode(&from));
    let destination = format!("http://{address}{prefix}{}", http_lite::percent_encode(&to));
    let response = http_lite::request(
        address,
        "MOVE",
        &target,
        &[("Destination", destination)],
        None,
        REQUEST_TIMEOUT,
    )
    .await
    .map_err(transport_error)?;
    match response.status {
        200..=299 => Ok(()),
        404 => Err(FsError::new("files.error.notFound", path)),
        409 => Err(FsError::new("files.error.notFound", to_dir)),
        status => Err(transport_error(format!("MOVE {path} -> HTTP {status}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_parsing_selects_http_addresses() {
        assert_eq!(
            parse_machine_id("fluidnc:WebSocket:192.168.1.50:80").as_deref(),
            Some("192.168.1.50:80")
        );
        // Port-less endpoints still get the default web port.
        assert_eq!(
            parse_machine_id("fluidnc:WebSocket:10.0.0.5").as_deref(),
            Some("10.0.0.5:80")
        );
        // Telnet discovery found the serial port; HTTP still lives on 80.
        assert_eq!(
            parse_machine_id("fluidnc:Telnet:192.168.1.50:23").as_deref(),
            Some("192.168.1.50:80")
        );
        assert_eq!(parse_machine_id("fluidnc:Serial:/dev/ttyUSB0"), None);
        assert_eq!(parse_machine_id("emulator:tcp_socket:127.0.0.1:8765"), None);
    }

    #[test]
    fn list_json_maps_directories_sizes_and_usage() {
        let raw = br#"{"files":[
            {"name":"sub dir","shortname":"SUBDIR","size":-1,"datetime":""},
            {"name":"a.gcode","shortname":"A.GCO","size":123,"datetime":""}
        ],"path":"/","total":"1048576","used":"524288","occupation":50,"status":"Ok"}"#;
        let list = parse_list_json(raw, FsSource::Controller, "/").expect("parse");
        assert_eq!(list.path, "/");
        assert_eq!(list.total_bytes, Some(1_048_576));
        assert_eq!(list.used_bytes, Some(524_288));
        assert_eq!(list.occupation_percent, Some(50));
        // Directories sort first despite alphabetical order.
        assert_eq!(list.entries[0].name, "sub dir");
        assert!(list.entries[0].is_dir);
        assert_eq!(list.entries[0].path, "/sub dir");
        assert_eq!(list.entries[1].size, 123);
    }

    #[test]
    fn list_json_accepts_numeric_usage_and_missing_sd() {
        let numeric =
            br#"{"files":[],"path":"/","total":99,"used":1,"occupation":0,"status":"Ok"}"#;
        let list = parse_list_json(numeric, FsSource::Controller, "/").expect("parse");
        assert_eq!(list.total_bytes, Some(99));
        assert!(list.entries.is_empty());

        let no_sd = br#"{"status":"No SD card"}"#;
        let error = parse_list_json(no_sd, FsSource::Sd, "/").expect_err("must fail");
        assert_eq!(error.key, "files.error.noSdCard");
    }

    /// Task (HTTP file access): firmware 4.1.1 answers the action API with
    /// every quantity as a STRING — sizes decimal, volume footer
    /// human-readable ("1.94 MB"). The parser must accept exactly this real
    /// payload, not only the numeric legacy shape.
    #[test]
    fn list_json_accepts_firmware_4x_string_footer() {
        let raw = br#"{"files":[
            {"name":"config.yaml","shortname":"config.yaml","size":"2401","datetime":""},
            {"name":"index.html.gz","shortname":"index.html.gz","size":"157736","datetime":""}
        ],"path":"","total":"1.94 MB","used":"168.00 KB","occupation":"8","status":"Ok"}"#;
        let list = parse_list_json(raw, FsSource::Controller, "/").expect("parse");
        assert_eq!(list.entries.len(), 2);
        // Case-insensitive alphabetical sort: config.yaml before index.html.gz.
        assert_eq!(list.entries[0].size, 2401);
        assert_eq!(list.entries[1].size, 157736);
        assert_eq!(
            list.total_bytes,
            Some((1.94_f64 * 1024.0 * 1024.0).round() as u64)
        );
        assert_eq!(list.used_bytes, Some((168.0_f64 * 1024.0).round() as u64));
        assert_eq!(list.occupation_percent, Some(8));
    }

    #[test]
    fn human_size_strings_convert_to_bytes() {
        assert_eq!(parse_size_string("2401"), Some(2401));
        assert_eq!(parse_size_string("168.00 KB"), Some(168 * 1024));
        assert_eq!(
            parse_size_string("1.94 MB"),
            Some((1.94_f64 * 1024.0 * 1024.0).round() as u64)
        );
        assert_eq!(parse_size_string("3 GB"), Some(3 * 1024 * 1024 * 1024));
        assert_eq!(parse_size_string("garbage"), None);
    }

    /// Task (HTTP file access): a PROPFIND multistatus with mixed tag-case,
    /// percent-escaped hrefs and a self entry must produce a correct listing
    /// with the ORIGINAL byte case preserved in names and paths.
    #[test]
    fn multistatus_lists_files_and_directories_case_insensitively() {
        let xml = r#"<?xml version="1.0"?>
<D:multistatus xmlns:D="DAV:">
<D:response><D:href>/flash/</D:href><D:propstat><D:prop><D:collection/></D:prop></D:propstat></D:response>
<d:response><href>/flash/My%20File.NC</href><propstat><prop><getcontentlength>1234</getcontentlength></prop></propstat></d:response>
<response><href>/flash/Sub</href><propstat><prop><collection/></prop></propstat></response>
</D:multistatus>"#;
        let list = parse_multistatus(xml, "/");
        assert_eq!(list.entries.len(), 2, "self directory must be skipped");
        // Directories sort first.
        assert_eq!(list.entries[0].name, "Sub");
        assert!(list.entries[0].is_dir);
        assert_eq!(list.entries[1].name, "My File.NC");
        assert!(!list.entries[1].is_dir);
        assert_eq!(list.entries[1].size, 1234);
        assert_eq!(list.entries[1].path, "/My File.NC");
    }

    #[test]
    fn percent_decode_roundtrips_escapes() {
        assert_eq!(percent_decode("/flash/My%20File.NC"), "/flash/My File.NC");
        assert_eq!(percent_decode("/flash/%C3%BCber"), "/flash/über");
        // Malformed escapes pass through untouched.
        assert_eq!(percent_decode("a%zzb"), "a%zzb");
    }

    #[test]
    fn remote_paths_are_normalized_or_rejected() {
        assert_eq!(
            remote_path("dir//sub/").as_deref().unwrap_or("?"),
            "/dir/sub"
        );
        assert!(matches!(
            remote_path("../x"),
            Err(e) if e.key == "files.error.badPath"
        ));
    }
}
