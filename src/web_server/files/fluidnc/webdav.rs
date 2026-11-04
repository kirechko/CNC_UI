//! FluidNC WebDAV file transfers and mutation operations.

use super::super::{http_lite, join_path, FsError, FsSource};
use super::{remote_path, transport_error, volume_prefix, REQUEST_TIMEOUT, TRANSFER_TIMEOUT};

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
