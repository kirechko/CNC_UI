//! Persistent user-macro operations backed by the application's SQLite database.

use super::AppState;
use crate::web_server::models::{MacroDefinition, MacroInvocation};
use rusqlite::{params, Connection, TransactionBehavior};

/// Reads the saved macro collection in stable name/id order from a database connection.
fn list_from_connection(connection: &Connection) -> Result<Vec<MacroDefinition>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, description, body, invocations
             FROM user_macros ORDER BY name COLLATE NOCASE, id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            let raw_invocations: String = row.get(4)?;
            let invocations = serde_json::from_str::<Vec<MacroInvocation>>(&raw_invocations)
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        raw_invocations.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            Ok(MacroDefinition {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                body: row.get(3)?,
                invocations,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Replaces the complete macro collection atomically on a database connection.
fn replace_in_connection(
    connection: &mut Connection,
    macros: &[MacroDefinition],
) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM user_macros", [])
        .map_err(|error| error.to_string())?;
    {
        let mut statement = transaction
            .prepare(
                "INSERT INTO user_macros (id, name, description, body, invocations)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(|error| error.to_string())?;
        for item in macros {
            let invocations =
                serde_json::to_string(&item.invocations).map_err(|error| error.to_string())?;
            statement
                .execute(params![
                    item.id,
                    item.name,
                    item.description,
                    item.body,
                    invocations
                ])
                .map_err(|error| error.to_string())?;
        }
    }
    transaction.commit().map_err(|error| error.to_string())
}

impl AppState {
    /// Loads all saved macros in stable name/id order.
    pub(crate) fn list_macros(&self) -> Result<Vec<MacroDefinition>, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "macro database lock poisoned".to_string())?;
        list_from_connection(&connection)
    }

    /// Replaces all saved macros atomically after validation in the API layer.
    pub(crate) fn replace_macros(&self, macros: &[MacroDefinition]) -> Result<(), String> {
        let mut connection = self
            .log
            .lock()
            .map_err(|_| "macro database lock poisoned".to_string())?;
        replace_in_connection(&mut connection, macros)
    }
}

/// Tests SQLite persistence and stable round-tripping of controller call metadata.
#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Creates an isolated database containing only the table used by these tests.
    fn test_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("open in-memory database");
        connection
            .execute_batch(
                "CREATE TABLE user_macros (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    description TEXT NOT NULL,
                    body TEXT NOT NULL,
                    invocations TEXT NOT NULL
                 );",
            )
            .expect("create macro table");
        connection
    }

    /// Verifies replacement and reload preserve each controller-specific invocation mapping.
    #[test]
    fn replaces_and_reads_macro_invocations() {
        let mut connection = test_connection();
        let definitions = vec![MacroDefinition {
            id: "fixture".into(),
            name: "Probe change".into(),
            description: "Probe the stock surface".into(),
            body: "G38.2 Z-5 F50".into(),
            invocations: vec![
                MacroInvocation {
                    controller: "LinuxCNC".into(),
                    mechanism: "M-code".into(),
                    command: "M101".into(),
                },
                MacroInvocation {
                    controller: "FluidNC".into(),
                    mechanism: "Custom command".into(),
                    command: "probe_stock".into(),
                },
            ],
        }];

        replace_in_connection(&mut connection, &definitions).expect("save macros");
        assert_eq!(
            list_from_connection(&connection).expect("load macros"),
            definitions
        );
        replace_in_connection(&mut connection, &[]).expect("clear macros");
        assert!(list_from_connection(&connection)
            .expect("load empty list")
            .is_empty());
    }
}
