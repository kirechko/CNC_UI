use rusqlite::{params, Connection};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

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

pub(crate) fn record(severity: &str, source: &str, message: &str) {
    if severity == "информация" {
        return;
    }

    if let Err(error) = try_record(severity, source, message) {
        eprintln!("Could not write message log: {error}");
    }
}

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

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64;
    connection.execute(
        "INSERT INTO messages (timestamp_ms, severity, source, message)
         VALUES (?1, ?2, ?3, ?4)",
        params![timestamp_ms, severity, source, message],
    )?;
    Ok(())
}
