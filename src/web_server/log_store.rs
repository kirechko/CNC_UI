//! Low-level rusqlite access for the SQLite database shared by the message
//! log, the machine-command history and the discovered-endpoint cache.
//!
//! `AppState` owns the single `Connection` behind a mutex and calls these
//! helpers; keeping the SQL here separates storage details from the
//! application logic in `state/`.

use super::models::{LatestLogMessages, LogEntry, LogPage, LogQuery};
use rusqlite::{params, Connection, OptionalExtension};
use std::fs;
use std::io;
use std::path::PathBuf;

const DEFAULT_LOG_PAGE_SIZE: usize = 100;
const MAX_LOG_PAGE_SIZE: usize = 500;

/// Builds the fatal startup error type used while opening the database.
fn startup_error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

/// Current unix time in milliseconds, clamped to `i64` range.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

/// Opens (and, if needed, creates) the message-log database. The location can
/// be overridden with `CNC_UI_LOG_DB`, which the UI tests use to isolate
/// their log state.
pub(crate) fn open_log() -> io::Result<Connection> {
    let path = std::env::var_os("CNC_UI_LOG_DB")
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
        });

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let connection = Connection::open(&path).map_err(|error| {
        startup_error(format!(
            "cannot open message log {}: {error}",
            path.display()
        ))
    })?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS messages (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 timestamp_ms INTEGER NOT NULL,
                 severity TEXT NOT NULL,
                 source TEXT NOT NULL,
                 message TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS messages_severity_id
                 ON messages(severity, id DESC);
             CREATE INDEX IF NOT EXISTS messages_id
                 ON messages(id DESC);
             CREATE TABLE IF NOT EXISTS command_history (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 command TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS command_history_id
                 ON command_history(id DESC);
             CREATE TABLE IF NOT EXISTS discovered_machine_cache (
                 endpoint TEXT PRIMARY KEY,
                 last_seen_ms INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS discovered_machine_cache_last_seen
                 ON discovered_machine_cache(last_seen_ms DESC);",
        )
        .map_err(|error| startup_error(format!("cannot initialize message log: {error}")))?;
    // Rewrite severities stored by older versions to the English identifiers.
    crate::message_log::migrate_legacy_severities(&connection)
        .map_err(|error| startup_error(format!("cannot migrate message log: {error}")))?;
    Ok(connection)
}

/// Inserts one message and prunes the table to the newest 10000 rows.
/// Informational and unknown severities are ignored by the caller.
pub(crate) fn insert_message(connection: &Connection, severity: &str, source: &str, message: &str) {
    let message: String = message.chars().take(2048).collect();
    if let Err(error) = connection.execute(
        "INSERT INTO messages (timestamp_ms, severity, source, message)
         VALUES (?1, ?2, ?3, ?4)",
        params![now_ms(), severity, source, message],
    ) {
        eprintln!("Could not write message log: {error}");
        return;
    }

    if let Err(error) = connection.execute(
        "DELETE FROM messages
         WHERE id NOT IN (SELECT id FROM messages ORDER BY id DESC LIMIT 10000)",
        [],
    ) {
        eprintln!("Could not prune message log: {error}");
    }
}

/// Newest alarm and newest non-alarm records for the header status chip.
pub(crate) fn latest_messages(connection: &Connection) -> Result<LatestLogMessages, String> {
    let critical = connection
        .query_row(
            "SELECT id, timestamp_ms, severity, source, message
             FROM messages WHERE severity = 'alarm' ORDER BY id DESC LIMIT 1",
            [],
            row_to_entry,
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let other = connection
        .query_row(
            "SELECT id, timestamp_ms, severity, source, message
             FROM messages WHERE severity != 'alarm' ORDER BY id DESC LIMIT 1",
            [],
            row_to_entry,
        )
        .optional()
        .map_err(|error| error.to_string())?;

    Ok(LatestLogMessages { critical, other })
}

/// Filters, sorts and pages the message log for the log dialog.
pub(crate) fn query(connection: &Connection, query: &LogQuery) -> Result<LogPage, String> {
    let severity = query.severity.as_deref().filter(|value| !value.is_empty());
    let source = query.source.as_deref().filter(|value| !value.is_empty());
    let search = query.search.as_deref().filter(|value| !value.is_empty());
    let time_from_ms = query.time_from_ms;
    let time_to_ms = query.time_to_ms;
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LOG_PAGE_SIZE)
        .clamp(1, MAX_LOG_PAGE_SIZE);
    let offset = query.offset.unwrap_or(0);
    let order = match query.sort.as_deref() {
        Some("severity") => "severity",
        Some("source") => "source",
        Some("message") => "message",
        Some("timestamp") => "timestamp_ms",
        _ => "id",
    };
    let direction = if query.descending.unwrap_or(true) {
        "DESC"
    } else {
        "ASC"
    };

    let total = connection
        .query_row(
            "SELECT COUNT(*) FROM messages
             WHERE (?1 IS NULL OR severity = ?1)
               AND (?2 IS NULL OR instr(source, ?2) > 0)
               AND (?3 IS NULL OR message LIKE '%' || ?3 || '%')
               AND (?4 IS NULL OR timestamp_ms >= ?4)
               AND (?5 IS NULL OR timestamp_ms <= ?5)",
            params![severity, source, search, time_from_ms, time_to_ms],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    // `order`/`direction` are whitelisted above, so interpolating them is safe.
    let sql = format!(
        "SELECT id, timestamp_ms, severity, source, message FROM messages
         WHERE (?1 IS NULL OR severity = ?1)
           AND (?2 IS NULL OR instr(source, ?2) > 0)
           AND (?3 IS NULL OR message LIKE '%' || ?3 || '%')
           AND (?4 IS NULL OR timestamp_ms >= ?4)
           AND (?5 IS NULL OR timestamp_ms <= ?5)
         ORDER BY {order} {direction}, id DESC LIMIT ?6 OFFSET ?7"
    );
    let mut statement = connection
        .prepare(&sql)
        .map_err(|error| error.to_string())?;
    let entries = statement
        .query_map(
            params![
                severity,
                source,
                search,
                time_from_ms,
                time_to_ms,
                limit,
                offset
            ],
            row_to_entry,
        )
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;

    Ok(LogPage { entries, total })
}

/// Maps a `messages` row to a [`LogEntry`].
fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<LogEntry> {
    Ok(LogEntry {
        id: row.get(0)?,
        timestamp_ms: row.get(1)?,
        severity: row.get(2)?,
        source: row.get(3)?,
        message: row.get(4)?,
    })
}

/// Ten most recently discovered endpoints, freshest first; the network scan
/// probes them with priority after a restart.
pub(crate) fn cached_endpoints(connection: &Connection) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT endpoint FROM discovered_machine_cache ORDER BY last_seen_ms DESC LIMIT 10",
        )
        .map_err(|error| error.to_string())?;
    // Collect eagerly into owned values so the mutable borrow of `statement`
    // ends here; keeping the lazy adapters as the tail expression extends the
    // borrow past the drop of `statement` at the end of the block (E0597).
    let rows = statement
        .query_map([], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    let collected: Vec<Result<String, rusqlite::Error>> = rows.collect();
    collected
        .into_iter()
        .map(|item| item.map_err(|error| error.to_string()))
        .collect()
}

/// Records one confirmed endpoint and prunes the cache to its ten newest rows.
pub(crate) fn cache_endpoint(connection: &Connection, endpoint: &str) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO discovered_machine_cache (endpoint, last_seen_ms) VALUES (?1, ?2)
             ON CONFLICT(endpoint) DO UPDATE SET last_seen_ms = excluded.last_seen_ms",
            params![endpoint, now_ms()],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "DELETE FROM discovered_machine_cache WHERE endpoint NOT IN
             (SELECT endpoint FROM discovered_machine_cache ORDER BY last_seen_ms DESC LIMIT 10)",
            [],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// The five hundred most recent machine commands, oldest first.
pub(crate) fn command_history(connection: &Connection) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT command FROM (
                 SELECT id, command FROM command_history
                 ORDER BY id DESC LIMIT 500
             ) ORDER BY id ASC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<String>, _>>()
        .map_err(|error| error.to_string())
}

/// Appends one command to the history unless it repeats the last entry.
pub(crate) fn record_command(connection: &Connection, command: &str) {
    let result = connection.execute(
        "INSERT INTO command_history (command)
         SELECT ?1
         WHERE NOT EXISTS (
             SELECT 1 FROM command_history
             ORDER BY id DESC LIMIT 1
         ) OR (
             SELECT command FROM command_history
             ORDER BY id DESC LIMIT 1
         ) != ?1",
        [command],
    );
    if let Err(error) = result {
        eprintln!("Could not store command history: {error}");
    } else if let Err(error) = connection.execute(
        "DELETE FROM command_history
         WHERE id NOT IN (
             SELECT id FROM command_history ORDER BY id DESC LIMIT 500
         )",
        [],
    ) {
        eprintln!("Could not prune command history: {error}");
    }
}
