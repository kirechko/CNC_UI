//! File-manager backend: browsing and mutating the local host file system and
//! the two FluidNC file systems (controller flash and SD card).
//!
//! The module is split for reviewability:
//!
//! - `local`    — host file system access jailed to `CNC_UI_FILES_ROOT`
//!   (or the process working directory when the variable is unset).
//! - `http_lite`— a minimal blocking-style HTTP/1.1 client over `tokio`
//!   TcpStream (the project deliberately has no reqwest dependency).
//! - `fluidnc`  — FluidNC protocol: JSON listing through the official
//!   `/files` and `/upload` action endpoints plus WebDAV mutations
//!   (`MKCOL`/`DELETE`/`MOVE`/`PUT`/`GET`) against `/flash` and `/sd`.
//!
//! Everything above those leaves is the source-agnostic dispatch in this file:
//! handlers call [`list`], [`read_bytes`], [`write_bytes`], [`make_dir`],
//! [`remove_path`], [`rename_path`], [`move_path`] and [`copy_path`], which
//! route to the right leaf and translate "cross-source" operations into
//! download + upload pairs (FluidNC has no server-side `COPY`).
//!
//! Errors travel as [`FsError`] (an i18n key plus a technical detail) and are
//! translated at the HTTP boundary by the handlers.

mod fluidnc;
mod http_lite;
mod local;

// Re-exported so the HTTP handlers can percent-encode download file names
// without reaching into the private `http_lite` module.
pub(crate) use http_lite::percent_encode;

use serde::Serialize;

use super::state::AppState;

/// Cap for text files served to the browser editor (mirrors the program size
/// limit); larger or non-UTF-8 content must go through the download endpoint.
pub(crate) const MAX_TEXT_BYTES: usize = 5 * 1024 * 1024;
/// Cap for a single upload request body accepted by `POST /api/files/upload`.
pub(crate) const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;

/// One of the three file systems the manager can talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FsSource {
    /// Host file system, jailed to the configured root directory.
    Local,
    /// FluidNC controller flash (LittleFS), WebDAV mount `/flash`.
    Controller,
    /// FluidNC SD card volume, WebDAV mount `/sd`.
    Sd,
}

impl FsSource {
    /// Parses the `source` query/body parameter (`local|controller|sd`).
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "controller" => Some(Self::Controller),
            "sd" => Some(Self::Sd),
            _ => None,
        }
    }

    /// Stable identifier sent to the browser.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Controller => "controller",
            Self::Sd => "sd",
        }
    }

    /// i18n key of the human-readable chip label.
    fn label_key(self) -> &'static str {
        match self {
            Self::Local => "files.source.local",
            Self::Controller => "files.source.controller",
            Self::Sd => "files.source.sd",
        }
    }
}

/// One directory entry; `path` is the full normalized path (`/dir/name`),
/// which is what every mutating endpoint expects.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileEntry {
    name: String,
    path: String,
    is_dir: bool,
    /// File size in bytes; `0` is reported for directories.
    size: u64,
    /// Last-modified timestamp in Unix milliseconds, `None` when the source
    /// does not expose it (FluidNC leaves the datetime column empty).
    #[serde(skip_serializing_if = "Option::is_none")]
    modified_ms: Option<u64>,
}

/// Result of a directory listing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileListResult {
    /// The directory that was listed, normalized (`/` or `/a/b`).
    path: String,
    /// Entries sorted directories-first, then case-insensitive by name.
    entries: Vec<FileEntry>,
    /// Volume capacity in bytes when the source reports it (FluidNC footer or
    /// the local file system via `statvfs` — task: show the free space of the
    /// filesystem displayed in the FILES panel).
    total_bytes: Option<u64>,
    /// Used space in bytes when the source reports it.
    used_bytes: Option<u64>,
    /// Usage percentage 0..=100 when the source reports it.
    occupation_percent: Option<u8>,
}

/// One selectable file-system source advertised to the browser.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FsSourceInfo {
    /// `"local" | "controller" | "sd"` — value for the `source` parameter.
    id: &'static str,
    /// Translated chip label.
    label: String,
    /// `false` when the source needs a FluidNC connection that is absent.
    available: bool,
}

/// Language-neutral file-manager error: an i18n key plus a technical detail
/// that the locale template can interpolate through `{detail}`.
#[derive(Debug)]
pub(crate) struct FsError {
    key: &'static str,
    detail: String,
}

impl FsError {
    pub(crate) fn new(key: &'static str, detail: impl Into<String>) -> Self {
        Self {
            key,
            detail: detail.into(),
        }
    }

    /// Renders the user-facing message in the requested language.
    pub(crate) fn translate(&self, language: &str) -> String {
        crate::i18n::translate(language, self.key, &[("detail", self.detail.clone())])
    }
}

/// Normalizes a client-supplied path into `/`-prefixed POSIX form.
///
/// Returns `None` for paths containing `..`, `.`, empty-but-spaced segments,
/// control characters or anything that is not a plain `/`-separated chain.
/// Missing and duplicate leading slashes are tolerated (`dir/a`, `/dir/a`).
pub(crate) fn normalize_path(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "/" {
        return Some("/".to_string());
    }
    let mut segments = Vec::new();
    for segment in raw.trim_start_matches('/').split('/') {
        if segment.is_empty() {
            // Collapse duplicate slashes but reject a spaced-only segment.
            continue;
        }
        if segment == "." || segment == ".." {
            return None;
        }
        if segment
            .chars()
            .any(|c| (c as u32) < 0x20 || c as u32 == 0x7f)
        {
            return None;
        }
        segments.push(segment);
    }
    if segments.is_empty() {
        return Some("/".to_string());
    }
    Some(format!("/{}", segments.join("/")))
}

/// Splits a normalized path into its directory and final component.
/// `"/"` has no name and yields `None`.
pub(crate) fn split_dir_and_name(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let name = trimmed.rsplit('/').next()?;
    if name.is_empty() {
        return None;
    }
    let dir = trimmed[..trimmed.len() - name.len()].trim_end_matches('/');
    Some((if dir.is_empty() { "/" } else { dir }, name))
}

/// Appends one name to a normalized directory path.
pub(crate) fn join_path(dir: &str, name: &str) -> String {
    if dir == "/" || dir.is_empty() {
        format!("/{name}")
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

/// Final path component of a normalized path (`""` for the root).
pub(crate) fn base_name(path: &str) -> &str {
    split_dir_and_name(path).map_or("", |(_, name)| name)
}

/// Lists the sources available for the current connection state.
pub(crate) fn sources(state: &AppState, language: &str) -> Vec<FsSourceInfo> {
    let fluidnc_ready = fluidnc::device_address(state).is_ok();
    [FsSource::Local, FsSource::Controller, FsSource::Sd]
        .into_iter()
        .map(|source| FsSourceInfo {
            id: source.as_str(),
            label: crate::i18n::translate(language, source.label_key(), &[]),
            available: source == FsSource::Local || fluidnc_ready,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Source-agnostic dispatch
// ---------------------------------------------------------------------------

/// Lists one directory on the requested source.
pub(crate) async fn list(
    state: &AppState,
    source: FsSource,
    path: &str,
) -> Result<FileListResult, FsError> {
    match source {
        FsSource::Local => local::list(path),
        source => fluidnc::list(&fluidnc::device_address(state)?, source, path).await,
    }
}

/// Reads a whole file (used by both the text editor and copy/move staging).
pub(crate) async fn read_bytes(
    state: &AppState,
    source: FsSource,
    path: &str,
) -> Result<Vec<u8>, FsError> {
    match source {
        FsSource::Local => local::read_bytes(path),
        source => fluidnc::read_bytes(&fluidnc::device_address(state)?, source, path).await,
    }
}

/// Writes (creates or overwrites) one file on the requested source.
pub(crate) async fn write_bytes(
    state: &AppState,
    source: FsSource,
    path: &str,
    data: &[u8],
) -> Result<(), FsError> {
    match source {
        FsSource::Local => local::write_bytes(path, data),
        source => fluidnc::write_bytes(&fluidnc::device_address(state)?, source, path, data).await,
    }
}

/// Creates one directory; parents must already exist.
pub(crate) async fn make_dir(
    state: &AppState,
    source: FsSource,
    path: &str,
) -> Result<(), FsError> {
    match source {
        FsSource::Local => local::mkdir(path),
        source => fluidnc::make_dir(&fluidnc::device_address(state)?, source, path).await,
    }
}

/// Removes a file or a whole directory recursively.
pub(crate) async fn remove_path(
    state: &AppState,
    source: FsSource,
    path: &str,
) -> Result<(), FsError> {
    match source {
        FsSource::Local => local::remove(path),
        source => fluidnc::remove(&fluidnc::device_address(state)?, source, path).await,
    }
}

/// Renames within the containing directory of the same source.
pub(crate) async fn rename_path(
    state: &AppState,
    source: FsSource,
    path: &str,
    new_name: &str,
) -> Result<(), FsError> {
    let (dir, _) =
        split_dir_and_name(path).ok_or_else(|| FsError::new("files.error.badPath", path))?;
    let target = join_path(dir, new_name);
    match source {
        FsSource::Local => local::rename(path, &target),
        source => {
            fluidnc::move_path(
                &fluidnc::device_address(state)?,
                source,
                path,
                dir,
                new_name,
            )
            .await
        }
    }
}

/// Moves into a different directory (possibly on a different source).
///
/// Same FluidNC volume uses WebDAV `MOVE`; anything else degrades to
/// copy + delete because FluidNC cannot move across volumes.
pub(crate) async fn move_path(
    state: &AppState,
    source: FsSource,
    path: &str,
    to_source: FsSource,
    to_dir: &str,
) -> Result<(), FsError> {
    let name = base_name(path);
    if name.is_empty() {
        return Err(FsError::new("files.error.badPath", path));
    }
    if source == to_source && source != FsSource::Local {
        return fluidnc::move_path(&fluidnc::device_address(state)?, source, path, to_dir, name)
            .await;
    }
    if source == to_source && source == FsSource::Local {
        return local::move_path(path, to_dir);
    }
    copy_path(state, source, path, to_source, to_dir).await?;
    remove_path(state, source, path).await
}

/// Copies into a different directory (possibly on a different source).
///
/// FluidNC advertises `COPY` but never dispatches it, so every remote copy is
/// a download + upload round-trip; only local-to-local uses `fs::copy`.
pub(crate) async fn copy_path(
    state: &AppState,
    source: FsSource,
    path: &str,
    to_source: FsSource,
    to_dir: &str,
) -> Result<(), FsError> {
    let name = base_name(path);
    if name.is_empty() {
        return Err(FsError::new("files.error.badPath", path));
    }
    if source == FsSource::Local && to_source == FsSource::Local {
        return local::copy_path(path, to_dir);
    }
    let data = read_bytes(state, source, path).await?;
    write_bytes(state, to_source, &join_path(to_dir, name), &data).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_accepts_plain_and_collapses_slashes() {
        assert_eq!(normalize_path("").as_deref(), Some("/"));
        assert_eq!(normalize_path("/").as_deref(), Some("/"));
        assert_eq!(normalize_path("/dir//sub/").as_deref(), Some("/dir/sub"));
        assert_eq!(normalize_path("dir/sub").as_deref(), Some("/dir/sub"));
    }

    #[test]
    fn normalize_path_rejects_traversal_and_controls() {
        assert_eq!(normalize_path("../etc"), None);
        assert_eq!(normalize_path("/a/../../b"), None);
        assert_eq!(normalize_path("/a/./b"), None);
        assert_eq!(normalize_path("/a\u{1}b"), None);
    }

    #[test]
    fn split_and_join_roundtrip() {
        assert_eq!(split_dir_and_name("/a/b.txt"), Some(("/a", "b.txt")));
        assert_eq!(split_dir_and_name("/b.txt"), Some(("/", "b.txt")));
        assert_eq!(split_dir_and_name("/"), None);
        assert_eq!(join_path("/", "x"), "/x");
        assert_eq!(join_path("/a", "x"), "/a/x");
        assert_eq!(base_name("/a/b/c"), "c");
        assert_eq!(base_name("/"), "");
    }

    #[test]
    fn source_parsing_is_case_insensitive_and_strict() {
        assert_eq!(FsSource::parse("Controller"), Some(FsSource::Controller));
        assert_eq!(FsSource::parse(" sd "), Some(FsSource::Sd));
        assert_eq!(FsSource::parse("flash"), None);
    }
}
