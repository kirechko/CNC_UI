use rusqlite::{params, Connection};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Internal severity identifier for critical alarms; stored in SQLite and sent over the API.
pub(crate) const SEVERITY_ALARM: &str = "alarm";
/// Internal severity identifier for errors; stored in SQLite and sent over the API.
pub(crate) const SEVERITY_ERROR: &str = "error";
/// Internal severity identifier for warnings; stored in SQLite and sent over the API.
pub(crate) const SEVERITY_WARNING: &str = "warning";
/// Internal severity identifier for informational events; stored in SQLite and sent over the API.
pub(crate) const SEVERITY_INFO: &str = "info";

/// Severities that are persisted to the message log (informational messages are not stored here).
pub(crate) const RECORDED_SEVERITIES: [&str; 3] = [SEVERITY_ALARM, SEVERITY_ERROR, SEVERITY_WARNING];

/// Every valid severity identifier accepted from the UI and API.
// The emulator binary includes this module without the UI API, so it does not use this list.
#[allow(dead_code)]
pub(crate) const ALL_SEVERITIES: [&str; 4] =
    [SEVERITY_ALARM, SEVERITY_ERROR, SEVERITY_WARNING, SEVERITY_INFO];

/// Legacy Russian severity labels written by earlier versions, mapped to their new identifiers.
const LEGACY_SEVERITY_MAP: [(&str, &str); 4] = [
    ("авария", SEVERITY_ALARM),
    ("ошибка", SEVERITY_ERROR),
    ("предупреждение", SEVERITY_WARNING),
    ("информация", SEVERITY_INFO),
];

/// Returns the path of the SQLite message log, honoring `CNC_UI_LOG_DB` when set.
///
/// The default location follows the XDG state directory convention.
fn database_path() -> PathBuf {
    std::env::var_os("CNC_UI_LOG_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let state_dir = std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .map(|home| home.join(".local").join("state"))
                })
                .unwrap_or_else(std::env::temp_dir);
            state_dir.join("linuxcnc-ui").join("messages.sqlite3")
        })
}

/// Rewrites severity values stored by older versions (Russian labels) to the English identifiers.
///
/// Runs on every start; after the first successful run it touches no rows.
/// Returns the number of rewritten rows.
pub(crate) fn migrate_legacy_severities(connection: &Connection) -> rusqlite::Result<usize> {
    let mut updated = 0;
    for (legacy, current) in LEGACY_SEVERITY_MAP {
        updated += connection.execute(
            "UPDATE messages SET severity = ?1 WHERE severity = ?2",
            params![current, legacy],
        )?;
    }
    Ok(updated)
}

/// Returns true when `severity` is one of the identifiers that is persisted to the log.
pub(crate) fn is_recorded_severity(severity: &str) -> bool {
    RECORDED_SEVERITIES.contains(&severity)
}

/// Appends a message to the persistent log; informational and unknown severities are ignored.
///
/// Errors are reported to stderr and never propagated, so logging cannot break control flow.
pub(crate) fn record(severity: &str, source: &str, message: &str) {
    if !is_recorded_severity(severity) {
        return;
    }

    if let Err(error) = try_record(severity, source, message) {
        eprintln!("Could not write message log: {error}");
    }
}

/// Opens the log database, ensures the schema, runs the legacy migration, and inserts one row.
fn try_record(severity: &str, source: &str, message: &str) -> rusqlite::Result<()> {
    let path = database_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    }

    let connection = Connection::open(path)?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS messages (
             id INTEGER PRIMARY KEY AUTOINCREMENT,
             timestamp_ms INTEGER NOT NULL,
             severity TEXT NOT NULL,
             source TEXT NOT NULL,
             message TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS messages_severity_id
             ON messages(severity, id DESC);
         CREATE INDEX IF NOT EXISTS messages_id
             ON messages(id DESC);",
    )?;
    migrate_legacy_severities(&connection)?;

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    let message: String = message.chars().take(2048).collect();
    connection.execute(
        "INSERT INTO messages (timestamp_ms, severity, source, message)
         VALUES (?1, ?2, ?3, ?4)",
        params![timestamp_ms, severity, source, message],
    )?;
    connection.execute(
        "DELETE FROM messages
         WHERE id NOT IN (SELECT id FROM messages ORDER BY id DESC LIMIT 10000)",
        [],
    )?;
    Ok(())
}
