//! Moonraker-backed file-manager operations for Klipper machines.

use super::{normalize_path, split_dir_and_name, FileEntry, FileListResult, FsError, FsSource};
use crate::web_server::state::AppState;

/// Returns whether the active controller adapter is Klipper/Moonraker.
pub(super) fn is_connected(state: &AppState) -> bool {
    let Ok(controller) = state.controller.lock() else {
        return false;
    };
    controller
        .discovery_api()
        .is_some_and(|api| api.klipper_adapter().is_some())
}

/// Clones the active Klipper adapter without unsafe trait-object pointer casts.
fn adapter(
    state: &AppState,
) -> Result<std::sync::Arc<cnc_core::adapters::klipper::KlipperAdapter>, FsError> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| FsError::new("files.error.notConnected", "controller lock poisoned"))?;
    let api = controller
        .discovery_api()
        .ok_or_else(|| FsError::new("files.error.notConnected", "no active adapter"))?;
    api.klipper_adapter()
        .cloned()
        .map(std::sync::Arc::new)
        .ok_or_else(|| FsError::new("files.error.notConnected", "active backend is not Klipper"))
}

/// Lists a normalized directory on Moonraker's `gcodes` or `config` root.
pub(super) async fn list(
    state: &AppState,
    source: FsSource,
    path: &str,
) -> Result<FileListResult, FsError> {
    let root = root(source)?;
    let path = normalize_path(path).ok_or_else(|| FsError::new("files.error.badPath", path))?;
    let entries = adapter(state)?
        .list_files(root, &path)
        .await
        .map_err(|e| transport_error(e.message))?;
    let entries = entries
        .into_iter()
        .map(|entry| FileEntry {
            name: entry.name.clone(),
            path: super::join_path(&path, &entry.name),
            is_dir: entry.is_dir,
            size: entry.size,
            modified_ms: None,
        })
        .collect();
    Ok(FileListResult {
        path,
        entries,
        total_bytes: None,
        used_bytes: None,
        occupation_percent: None,
    })
}

/// Downloads one file from Moonraker's matching root.
pub(super) async fn read_bytes(
    state: &AppState,
    source: FsSource,
    path: &str,
) -> Result<Vec<u8>, FsError> {
    let root = root(source)?;
    let path = normalize_path(path).ok_or_else(|| FsError::new("files.error.badPath", path))?;
    adapter(state)?
        .download_file(root, &path)
        .await
        .map_err(|e| transport_error(e.message))
}

/// Uploads or overwrites a file through Moonraker's multipart REST API.
pub(super) async fn write_bytes(
    state: &AppState,
    source: FsSource,
    path: &str,
    data: &[u8],
) -> Result<(), FsError> {
    let root = root(source)?;
    let path = normalize_path(path).ok_or_else(|| FsError::new("files.error.badPath", path))?;
    split_dir_and_name(&path).ok_or_else(|| FsError::new("files.error.badPath", path.clone()))?;
    adapter(state)?
        .upload_bytes(root, &path.trim_start_matches('/'), data)
        .await
        .map_err(|e| transport_error(e.message))
}

/// Moves a file or directory within one Moonraker root.
pub(super) async fn move_path(
    state: &AppState,
    source: FsSource,
    path: &str,
    to_dir: &str,
    name: &str,
) -> Result<(), FsError> {
    let root = root(source)?;
    let path = normalize_path(path).ok_or_else(|| FsError::new("files.error.badPath", path))?;
    let to_dir =
        normalize_path(to_dir).ok_or_else(|| FsError::new("files.error.badPath", to_dir))?;
    let destination = if to_dir == "/" {
        name.to_owned()
    } else {
        format!("{to_dir}/{name}")
    };
    adapter(state)?
        .move_file(root, &path, &destination)
        .await
        .map_err(|e| transport_error(e.message))
}

/// Removes one Moonraker file or directory.
pub(super) async fn remove(state: &AppState, source: FsSource, path: &str) -> Result<(), FsError> {
    let root = root(source)?;
    let path = normalize_path(path).ok_or_else(|| FsError::new("files.error.badPath", path))?;
    adapter(state)?
        .delete_file(root, &path)
        .await
        .map_err(|e| transport_error(e.message))
}

/// Maps UI file sources onto Moonraker roots.
fn root(source: FsSource) -> Result<&'static str, FsError> {
    match source {
        FsSource::Controller => Ok("gcodes"),
        FsSource::Sd => Ok("config"),
        FsSource::Local => Err(FsError::new(
            "files.error.badSource",
            "local filesystem is not Moonraker",
        )),
    }
}

/// Wraps Moonraker failures in the file-manager's translated remote error.
fn transport_error(detail: String) -> FsError {
    FsError::new("files.error.deviceHttp", detail)
}
