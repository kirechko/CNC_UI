//! Host file system access for the file manager.
//!
//! Every operation is jailed to a single root directory: the value of
//! `CNC_UI_FILES_ROOT` when set, otherwise the current working directory of
//! the backend process. Relative manager paths (`/dir/file`) are resolved
//! under that root; the resolved path is canonicalized and must still start
//! with the canonical root, which blocks both `..` traversal (already rejected
//! by `normalize_path`) and symlink escapes.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use super::{base_name, normalize_path, split_dir_and_name, FsError, FileEntry, FileListResult};

/// Refuse to list directories with more entries than this; protects both the
/// browser and the JSON payload from monster directories.
const MAX_LIST_ENTRIES: usize = 2000;

/// The jail root. Canonical when possible; falls back to the raw value so a
/// not-yet-existing configured root still produces `notFound` errors instead
/// of panicking at startup.
fn root() -> PathBuf {
    let configured = std::env::var("CNC_UI_FILES_ROOT")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        });
    configured.canonicalize().unwrap_or(configured)
}

/// Maps an OS error to an `FsError` whose detail carries the OS message plus
/// the failing path for support diagnostics.
fn io_error(operation: &str, error: io::Error) -> FsError {
    FsError::new("files.error.io", format!("{operation}: {error}"))
}

/// Resolves a normalized manager path to an absolute path inside the jail.
///
/// `None` for malformed paths; `Some(Err(denied))` when the joined path
/// escapes the root (checked textually here — the canonicalized re-check in
/// [`existing`] is what defeats symlinks).
fn jailed(rel: &str) -> Result<PathBuf, FsError> {
    let normalized = normalize_path(rel).ok_or_else(|| FsError::new("files.error.badPath", rel))?;
    let mut path = root();
    for segment in normalized.trim_start_matches('/').split('/').filter(|s| !s.is_empty()) {
        path.push(segment);
    }
    let prefix = root();
    if !path.starts_with(&prefix) {
        return Err(FsError::new("files.error.denied", rel));
    }
    Ok(path)
}

/// Canonicalizes an existing jail path and re-validates the prefix. This is
/// the choke point that turns symlinks and `..` into a denial.
fn existing(rel: &str) -> Result<PathBuf, FsError> {
    let candidate = jailed(rel)?;
    let resolved = candidate
        .canonicalize()
        .map_err(|_| FsError::new("files.error.notFound", rel))?;
    let prefix = root().canonicalize().unwrap_or_else(|_| root());
    if !resolved.starts_with(&prefix) {
        return Err(FsError::new("files.error.denied", rel));
    }
    Ok(resolved)
}

/// Builds the path of a not-yet-existing child of an existing directory: the
/// directory itself goes through [`existing`], then the name is appended.
/// Guards mutations whose own target does not exist yet (create/rename dest).
fn parented(rel: &str) -> Result<PathBuf, FsError> {
    let (dir, name) = split_dir_and_name(rel).ok_or_else(|| FsError::new("files.error.badPath", rel))?;
    Ok(existing(dir)?.join(name))
}

/// Unix milliseconds of a modification time, `None` for pre-epoch stamps.
fn modified_ms(metadata: &fs::Metadata) -> Option<u64> {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as u64)
}

/// One `read_dir` row into a manager entry rooted at the listed directory.
fn entry_at(dir_rel: &str, dent: &fs::DirEntry, metadata: &fs::Metadata) -> FileEntry {
    let name = dent.file_name().to_string_lossy().into_owned();
    let is_dir = metadata.is_dir();
    FileEntry {
        is_dir,
        size: if is_dir { 0 } else { metadata.len() },
        modified_ms: modified_ms(metadata),
        path: super::join_path(dir_rel, &name),
        name,
    }
}

/// Lists one directory inside the jail, sorted directories-first then by
/// lowercased name for a stable, human-friendly order.
pub(crate) fn list(rel: &str) -> Result<FileListResult, FsError> {
    let normalized = normalize_path(rel).ok_or_else(|| FsError::new("files.error.badPath", rel))?;
    let dir = existing(&normalized)?;
    let mut entries: Vec<FileEntry> = Vec::new();
    let read = fs::read_dir(&dir).map_err(|error| io_error("read_dir", error))?;
    for dent in read.take(MAX_LIST_ENTRIES + 1) {
        let dent = dent.map_err(|error| io_error("read_dir", error))?;
        let metadata = dent
            .metadata()
            .map_err(|error| io_error("metadata", error))?;
        entries.push(entry_at(&normalized, &dent, &metadata));
    }
    if entries.len() > MAX_LIST_ENTRIES {
        return Err(FsError::new(
            "files.error.io",
            format!("too many entries (> {MAX_LIST_ENTRIES})"),
        ));
    }
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(FileListResult {
        path: normalized,
        entries,
        total_bytes: None,
        used_bytes: None,
        occupation_percent: None,
    })
}

/// Reads a whole file; size is bounded later by the handler that needs it.
pub(crate) fn read_bytes(rel: &str) -> Result<Vec<u8>, FsError> {
    let path = existing(rel)?;
    if path.is_dir() {
        return Err(FsError::new("files.error.badPath", rel));
    }
    fs::read(&path).map_err(|error| io_error("read", error))
}

/// Creates or truncates one file under an existing directory.
pub(crate) fn write_bytes(rel: &str, data: &[u8]) -> Result<(), FsError> {
    let path = parented(rel)?;
    if path.exists() && path.is_dir() {
        return Err(FsError::new("files.error.badPath", rel));
    }
    fs::write(&path, data).map_err(|error| io_error("write", error))
}

/// Creates one directory; fails with `alreadyExists` when the name is taken.
pub(crate) fn mkdir(rel: &str) -> Result<(), FsError> {
    let path = parented(rel)?;
    if path.exists() {
        return Err(FsError::new("files.error.alreadyExists", rel));
    }
    fs::create_dir(&path).map_err(|error| io_error("create_dir", error))
}

/// Removes a file or a whole directory subtree.
pub(crate) fn remove(rel: &str) -> Result<(), FsError> {
    let path = existing(rel)?;
    let result = if path.is_dir() {
        fs::remove_dir_all(&path)
    } else {
        fs::remove_file(&path)
    };
    result.map_err(|error| io_error("remove", error))
}

/// Renames within the same directory tree (target must not exist).
pub(crate) fn rename(rel: &str, target_rel: &str) -> Result<(), FsError> {
    let from = existing(rel)?;
    let to = parented(target_rel)?;
    if to.exists() {
        return Err(FsError::new("files.error.alreadyExists", target_rel));
    }
    fs::rename(&from, &to).map_err(|error| io_error("rename", error))
}

/// Moves an entry into another directory of the same jail (same filesystem,
/// so `fs::rename` is enough; a conflicting name is refused).
pub(crate) fn move_path(rel: &str, to_dir_rel: &str) -> Result<(), FsError> {
    let name = base_name(rel);
    if name.is_empty() {
        return Err(FsError::new("files.error.badPath", rel));
    }
    let from = existing(rel)?;
    let to = parented(&super::join_path(to_dir_rel, name))?;
    if to.exists() {
        return Err(FsError::new("files.error.alreadyExists", name));
    }
    fs::rename(&from, &to).map_err(|error| io_error("move", error))
}

/// Copies a *file* into another directory (`fs::copy`); directories would need
/// a recursive walk that FluidNC cannot mirror anyway, so they are refused.
pub(crate) fn copy_path(rel: &str, to_dir_rel: &str) -> Result<(), FsError> {
    let name = base_name(rel);
    if name.is_empty() {
        return Err(FsError::new("files.error.badPath", rel));
    }
    let from = existing(rel)?;
    if from.is_dir() {
        return Err(FsError::new("files.error.dirCopyUnsupported", rel));
    }
    let to = parented(&super::join_path(to_dir_rel, name))?;
    if to.exists() {
        return Err(FsError::new("files.error.alreadyExists", name));
    }
    fs::copy(&from, &to).map_err(|error| io_error("copy", error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// `CNC_UI_FILES_ROOT` is process-global, so every jail test must run
    /// while this lock is held; otherwise cargo's parallel test threads
    /// would repoint each other's roots mid-assertion.
    static JAIL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    /// Creates a uniquely named temporary directory and points the jail
    /// environment at it; returns the directory path for assertions.
    fn temp_root(tag: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.subsec_nanos())
            .unwrap_or(0);
        path.push(format!("cnc-ui-files-{tag}-{nanos}"));
        fs::create_dir_all(&path).expect("create temp root");
        std::env::set_var("CNC_UI_FILES_ROOT", &path);
        path.canonicalize().expect("canonical temp root")
    }

    /// Holds the jail lock for the whole test body (poisoned locks are fine:
    /// the only guarded data is the environment variable and temp dirs).
    fn lock_jail() -> std::sync::MutexGuard<'static, ()> {
        JAIL_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn local_crud_roundtrip() {
        let _guard = lock_jail();
        let _root = temp_root("crud");
        mkdir("/docs").expect("mkdir");
        assert!(matches!(mkdir("/docs"), Err(e) if e.key == "files.error.alreadyExists"));
        write_bytes("/docs/a.txt", b"hello").expect("write");
        assert_eq!(read_bytes("/docs/a.txt").expect("read"), b"hello");
        let listed = list("/").expect("list root");
        assert_eq!(listed.entries.len(), 1);
        assert!(listed.entries[0].is_dir);
        assert_eq!(listed.entries[0].path, "/docs");
        let inner = list("/docs").expect("list docs");
        assert_eq!(inner.entries[0].name, "a.txt");
        assert_eq!(inner.entries[0].size, 5);
        assert!(inner.entries[0].modified_ms.is_some());

        rename("/docs/a.txt", "/docs/b.txt").expect("rename");
        move_path("/docs/b.txt", "/").expect("move to root");
        assert!(matches!(
            read_bytes("/docs/b.txt"),
            Err(e) if e.key == "files.error.notFound"
        ));
        copy_path("/b.txt", "/docs").expect("copy");
        assert_eq!(read_bytes("/docs/b.txt").expect("copy read"), b"hello");
        remove("/docs").expect("remove dir");
        assert!(matches!(
            list("/docs"),
            Err(e) if e.key == "files.error.notFound"
        ));
    }

    #[test]
    fn jail_blocks_traversal_and_absolute_paths() {
        let _guard = lock_jail();
        let _root = temp_root("jail");
        // `normalize_path` rejects `..` before any filesystem is touched.
        assert!(matches!(
            read_bytes("/../secret"),
            Err(e) if e.key == "files.error.badPath"
        ));
        assert!(matches!(
            list("/a/../../b"),
            Err(e) if e.key == "files.error.badPath"
        ));
        // Backslash-heavy windows-style input is just an odd but valid name;
        // it must never escape because segments are joined, not parsed.
        assert!(jailed("/..%2f..").is_ok());
    }

    #[test]
    fn rename_refuses_existing_target() {
        let _guard = lock_jail();
        let _root = temp_root("clash");
        write_bytes("/one.txt", b"1").expect("write one");
        write_bytes("/two.txt", b"2").expect("write two");
        assert!(matches!(
            rename("/one.txt", "/two.txt"),
            Err(e) if e.key == "files.error.alreadyExists"
        ));
    }

    #[test]
    fn directory_copy_is_refused() {
        let _guard = lock_jail();
        let _root = temp_root("dircopy");
        mkdir("/box").expect("mkdir");
        assert!(matches!(
            copy_path("/box", "/"),
            Err(e) if e.key == "files.error.dirCopyUnsupported"
        ));
    }
}
