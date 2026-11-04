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

mod address;
mod listing;
mod webdav;

pub(crate) use address::device_address;
pub(crate) use listing::list;
pub(crate) use webdav::{make_dir, move_path, read_bytes, remove, write_bytes};

use std::time::Duration;

use super::{normalize_path, FsError, FsSource};

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

/// Wraps transport failures into the `deviceHttp` i18n key.
fn transport_error(detail: String) -> FsError {
    FsError::new("files.error.deviceHttp", detail)
}

pub(super) fn remote_path(path: &str) -> Result<String, FsError> {
    normalize_path(path).ok_or_else(|| FsError::new("files.error.badPath", path))
}
