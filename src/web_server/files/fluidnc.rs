//! FluidNC controller file systems over plain HTTP.
//!
//! Two official interfaces are combined (verified against the upstream
//! `WebUIServer.cpp` / `WebDAV.cpp` sources):
//!
//! - **Listing** uses the action API: `GET /files?...` for the flash volume
//!   and `GET /upload?...` for the SD card. The JSON footer carries volume
//!   usage; entries with `size == -1` are directories.
//! - **Everything else** uses the WebDAV mounts `/flash` and `/sd`, because
//!   WebDAV status codes are deterministic while the action API returns
//!   human-readable text: `MKCOL` (201/405), `DELETE` (204/404/413),
//!   `MOVE` (2xx, same volume only), `PUT` (201/507) and `GET` (200/404/503).
//!   `COPY` is advertised in `Allow` but never dispatched, so cross-volume
//!   copies are performed by the dispatcher as download + upload.
//!
//! The HTTP port is always the controller's web port (80 by default); a
//! Telnet-discovered device still exposes the same server, so the endpoint
//! port from discovery is only trusted for the WebSocket transport.

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
/// machine id (`fluidnc:<Transport>:<endpoint>`). Non-network transports
/// (`Serial`, `UnixSocket`) have no HTTP server and yield `None`.
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
pub(crate) fn device_address(state: &AppState) -> Result<String, FsError> {
    let id = state
        .connected_machine_id()
        .ok_or_else(|| FsError::new("files.error.notConnected", ""))?;
    if !id.starts_with("fluidnc:") {
        return Err(FsError::new("files.error.notFluidNc", ""));
    }
    parse_machine_id(&id).ok_or_else(|| FsError::new("files.error.notFluidNc", id))
}

/// Wraps transport failures into the `deviceHttp` i18n key.
fn transport_error(detail: String) -> FsError {
    FsError::new("files.error.deviceHttp", detail)
}

/// One numeric-or-string JSON quantity (FluidNC prints `total`/`used` as
/// strings but older builds may send numbers).
fn bytes_value(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
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
        // FluidNC reports -1 for directories.
        let raw_size = item.get("size").and_then(Value::as_i64).unwrap_or(0);
        let is_dir = raw_size < 0;
        entries.push(FileEntry {
            name: name.to_string(),
            path: join_path(dir, name),
            is_dir,
            size: if is_dir { 0 } else { raw_size as u64 },
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
        occupation_percent: parsed
            .get("occupation")
            .and_then(Value::as_u64)
            .map(|value| value.min(u8::MAX as u64) as u8),
    })
}

/// Validates a normalized manager path for use on a FluidNC volume.
fn remote_path(path: &str) -> Result<String, FsError> {
    normalize_path(path).ok_or_else(|| FsError::new("files.error.badPath", path))
}

/// Lists one directory on a FluidNC volume via the action API.
pub(crate) async fn list(
    address: &str,
    source: FsSource,
    path: &str,
) -> Result<FileListResult, FsError> {
    let dir = remote_path(path)?;
    let target = format!(
        "{}?action=list&filename=all&path={}",
        action_endpoint(source),
        http_lite::percent_encode(&dir)
    );
    let response = http_lite::request(address, "GET", &target, &[], None, REQUEST_TIMEOUT)
        .await
        .map_err(transport_error)?;
    if response.status != 200 {
        return Err(transport_error(format!(
            "listing HTTP status {}",
            response.status
        )));
    }
    parse_list_json(&response.body, source, &dir)
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
        assert_eq!(parse_machine_id("emulator:unix_socket:/tmp/x.sock"), None);
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
