//! Persistent macro and folder operations backed by SQLite.

use super::AppState;
use crate::web_server::models::{MacroDefinition, MacroFolder, MacroInvocation};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

/// Reads macros in deterministic folder/name order, preserving parent folder placement.
fn list_from_connection(connection: &Connection) -> Result<Vec<MacroDefinition>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, description, body, invocations, folder_id, tags
         FROM user_macros ORDER BY COALESCE(folder_id, ''), name COLLATE NOCASE, id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            let raw: String = row.get(4)?;
            let invocations =
                serde_json::from_str::<Vec<MacroInvocation>>(&raw).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        raw.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            let raw_tags: String = row.get(6)?;
            let tags = serde_json::from_str::<Vec<String>>(&raw_tags).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    raw_tags.len(),
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
                folder_id: row.get(5)?,
                tags,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Replaces the macro collection atomically, refusing references to nonexistent folders.
fn replace_in_connection(
    connection: &mut Connection,
    macros: &[MacroDefinition],
) -> Result<(), String> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    tx.execute("DELETE FROM user_macros", [])
        .map_err(|error| error.to_string())?;
    {
        let mut insert = tx.prepare("INSERT INTO user_macros (id,name,description,body,invocations,folder_id,tags) VALUES (?1,?2,?3,?4,?5,?6,?7)")
            .map_err(|error| error.to_string())?;
        for item in macros {
            if let Some(folder_id) = item.folder_id.as_deref() {
                let exists: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM macro_folders WHERE id=?1)",
                        [folder_id],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                if !exists {
                    return Err(format!("macro folder `{folder_id}` does not exist"));
                }
            }
            let invocations =
                serde_json::to_string(&item.invocations).map_err(|error| error.to_string())?;
            let tags = serde_json::to_string(&item.tags).map_err(|error| error.to_string())?;
            insert
                .execute(params![
                    item.id,
                    item.name,
                    item.description,
                    item.body,
                    invocations,
                    item.folder_id,
                    tags
                ])
                .map_err(|error| error.to_string())?;
        }
    }
    tx.commit().map_err(|error| error.to_string())
}

/// Creates tables and upgrades legacy macro databases by adding folder placement and tags.
pub(crate) fn ensure_folder_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS macro_folders (
             id TEXT PRIMARY KEY,
             name TEXT NOT NULL,
             parent_id TEXT REFERENCES macro_folders(id) ON DELETE RESTRICT
         );
         CREATE UNIQUE INDEX IF NOT EXISTS macro_folders_sibling_name
             ON macro_folders(COALESCE(parent_id, ''), name COLLATE NOCASE);",
        )
        .map_err(|error| format!("cannot create macro folder schema: {error}"))?;
    let has_column: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('user_macros') WHERE name='folder_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("cannot inspect macro schema: {error}"))?;
    if has_column == 0 {
        connection.execute("ALTER TABLE user_macros ADD COLUMN folder_id TEXT REFERENCES macro_folders(id) ON DELETE SET NULL", [])
            .map_err(|error| format!("cannot add macro folder reference: {error}"))?;
    }
    let has_tags: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('user_macros') WHERE name='tags'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("cannot inspect macro tags schema: {error}"))?;
    if has_tags == 0 {
        connection
            .execute(
                "ALTER TABLE user_macros ADD COLUMN tags TEXT NOT NULL DEFAULT '[]'",
                [],
            )
            .map_err(|error| format!("cannot add macro tags: {error}"))?;
    }
    Ok(())
}

/// Lists the folder hierarchy as flat nodes carrying their parent ids.
fn list_folders_from_connection(connection: &Connection) -> Result<Vec<MacroFolder>, String> {
    let mut statement = connection
        .prepare("SELECT id,name,parent_id FROM macro_folders ORDER BY name COLLATE NOCASE,id")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(MacroFolder {
                id: row.get(0)?,
                name: row.get(1)?,
                parent_id: row.get(2)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Creates a folder only after validating its existing ancestor chain and preventing cycles.
fn create_folder_in_connection(
    connection: &mut Connection,
    folder: &MacroFolder,
) -> Result<(), String> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    if let Some(parent_id) = folder.parent_id.as_deref() {
        let mut current = Some(parent_id.to_owned());
        let mut seen = std::collections::HashSet::new();
        while let Some(id) = current {
            if id == folder.id || !seen.insert(id.clone()) {
                return Err("folder parent cycle is not allowed".into());
            }
            current = tx
                .query_row(
                    "SELECT parent_id FROM macro_folders WHERE id=?1",
                    [&id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "parent folder does not exist".to_string())?;
        }
    }
    tx.execute(
        "INSERT INTO macro_folders (id,name,parent_id) VALUES (?1,?2,?3)",
        params![folder.id, folder.name, folder.parent_id],
    )
    .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())
}

impl AppState {
    /// Loads all user macros in stable order.
    pub(crate) fn list_macros(&self) -> Result<Vec<MacroDefinition>, String> {
        let db = self
            .log
            .lock()
            .map_err(|_| "macro database lock poisoned".to_string())?;
        list_from_connection(&db)
    }

    /// Loads folders for breadcrumb/tree rendering.
    pub(crate) fn list_macro_folders(&self) -> Result<Vec<MacroFolder>, String> {
        let db = self
            .log
            .lock()
            .map_err(|_| "macro database lock poisoned".to_string())?;
        list_folders_from_connection(&db)
    }

    /// Creates a folder under a root or existing parent node.
    pub(crate) fn create_macro_folder(&self, folder: &MacroFolder) -> Result<(), String> {
        let mut db = self
            .log
            .lock()
            .map_err(|_| "macro database lock poisoned".to_string())?;
        create_folder_in_connection(&mut db, folder)
    }

    /// Deletes an empty folder transactionally; nonempty folders are retained to prevent data loss.
    pub(crate) fn delete_macro_folder(&self, folder_id: &str) -> Result<(), String> {
        let mut db = self
            .log
            .lock()
            .map_err(|_| "macro database lock poisoned".to_string())?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| error.to_string())?;
        let count: i64 = tx.query_row("SELECT (SELECT COUNT(*) FROM macro_folders WHERE parent_id=?1)+(SELECT COUNT(*) FROM user_macros WHERE folder_id=?1)", [folder_id], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if count != 0 {
            return Err("folder is not empty".into());
        }
        if tx
            .execute("DELETE FROM macro_folders WHERE id=?1", [folder_id])
            .map_err(|error| error.to_string())?
            == 0
        {
            return Err("folder does not exist".into());
        }
        tx.commit().map_err(|error| error.to_string())
    }

    /// Replaces all macro entries in one transaction.
    pub(crate) fn replace_macros(&self, macros: &[MacroDefinition]) -> Result<(), String> {
        let mut db = self
            .log
            .lock()
            .map_err(|_| "macro database lock poisoned".to_string())?;
        replace_in_connection(&mut db, macros)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates an isolated database with the production folder and tag constraints.
    fn test_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("open in-memory database");
        connection.execute_batch("CREATE TABLE user_macros (id TEXT PRIMARY KEY,name TEXT NOT NULL,description TEXT NOT NULL,body TEXT NOT NULL,invocations TEXT NOT NULL,folder_id TEXT,tags TEXT NOT NULL DEFAULT '[]'); CREATE TABLE macro_folders (id TEXT PRIMARY KEY,name TEXT NOT NULL,parent_id TEXT); CREATE UNIQUE INDEX macro_folders_sibling_name ON macro_folders(COALESCE(parent_id, ''), name COLLATE NOCASE);").expect("create test schema");
        connection
    }

    /// Confirms folders persist in tree order and macros round-trip their folder association.
    #[test]
    fn folder_hierarchy_and_macro_placement_round_trip() {
        let mut connection = test_connection();
        create_folder_in_connection(
            &mut connection,
            &MacroFolder {
                id: "root".into(),
                name: "Shop".into(),
                parent_id: None,
            },
        )
        .unwrap();
        create_folder_in_connection(
            &mut connection,
            &MacroFolder {
                id: "child".into(),
                name: "Probe".into(),
                parent_id: Some("root".into()),
            },
        )
        .unwrap();
        let macro_item = MacroDefinition {
            id: "m1".into(),
            name: "Zero".into(),
            description: String::new(),
            body: "G92 Z0".into(),
            tags: vec!["probe".into()],
            invocations: Vec::new(),
            folder_id: Some("child".into()),
        };
        replace_in_connection(&mut connection, &[macro_item.clone()]).unwrap();
        assert_eq!(list_folders_from_connection(&connection).unwrap().len(), 2);
        assert_eq!(list_from_connection(&connection).unwrap(), vec![macro_item]);
    }

    /// Rejects non-existent parents and protects non-empty folders from deletion.
    #[test]
    fn folder_parent_and_delete_guards_reject_invalid_tree_operations() {
        let mut connection = test_connection();
        assert!(create_folder_in_connection(
            &mut connection,
            &MacroFolder {
                id: "bad".into(),
                name: "Bad".into(),
                parent_id: Some("missing".into())
            }
        )
        .is_err());
        create_folder_in_connection(
            &mut connection,
            &MacroFolder {
                id: "root".into(),
                name: "Root".into(),
                parent_id: None,
            },
        )
        .unwrap();
        create_folder_in_connection(
            &mut connection,
            &MacroFolder {
                id: "child".into(),
                name: "Child".into(),
                parent_id: Some("root".into()),
            },
        )
        .unwrap();
        assert!(create_folder_in_connection(
            &mut connection,
            &MacroFolder {
                id: "root".into(),
                name: "Cycle".into(),
                parent_id: Some("child".into())
            }
        )
        .is_err());
        assert!(create_folder_in_connection(
            &mut connection,
            &MacroFolder {
                id: "duplicate".into(),
                name: "root".into(),
                parent_id: None
            }
        )
        .is_err());
        let macro_item = MacroDefinition {
            id: "m".into(),
            name: "M".into(),
            description: String::new(),
            body: "G0".into(),
            tags: Vec::new(),
            invocations: Vec::new(),
            folder_id: Some("root".into()),
        };
        replace_in_connection(&mut connection, &[macro_item]).unwrap();
        assert!(replace_in_connection(
            &mut connection,
            &[MacroDefinition {
                id: "badref".into(),
                name: "Bad ref".into(),
                description: String::new(),
                body: "G0".into(),
                tags: Vec::new(),
                invocations: Vec::new(),
                folder_id: Some("missing".into())
            }]
        )
        .is_err());
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM user_macros", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "failed replacement rolls back and preserves prior macros"
        );
    }
}
