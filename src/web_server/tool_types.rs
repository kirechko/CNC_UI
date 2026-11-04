//! SQLite persistence and validation for user-extensible tool categories.
//!
//! Geometry values are constrained descriptors; arbitrary mesh uploads and estimation stay out of scope.
//! This is deliberate until orientation, units, parser limits, and geometry-derived diameter semantics are defined.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

/// User-defined tool category with advisory cutting and display-geometry metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolTypeRecord {
    /// Stable local identifier referenced by tool records.
    pub id: String,
    /// User-visible category name.
    pub name: String,
    /// Process family: milling, drilling, laser, probing, or other.
    pub cutting_type: String,
    /// Built-in geometry descriptor; this is not an uploaded or estimated mesh.
    pub geometry_model: String,
}

/// Initializes the separate tool-type table and seeds common editable categories once.
pub fn ensure_schema(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS tool_types (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL COLLATE NOCASE UNIQUE,
            record_json TEXT NOT NULL
         );",
        )
        .map_err(|error| format!("cannot initialize tool types schema: {error}"))?;
    for (id, name, cutting_type, geometry_model) in [
        ("end-mill", "End mill", "milling", "cylindrical"),
        ("ball-nose", "Ball nose", "milling", "ball_nose"),
        ("v-bit", "V-bit", "milling", "v_bit"),
        ("drill", "Drill", "drilling", "drill"),
        ("laser", "Laser head", "laser", "laser_spot"),
        ("probe", "Probe", "probing", "probe_tip"),
    ] {
        let record = ToolTypeRecord {
            id: id.into(),
            name: name.into(),
            cutting_type: cutting_type.into(),
            geometry_model: geometry_model.into(),
        };
        let json = serde_json::to_string(&record).map_err(|error| error.to_string())?;
        connection
            .execute(
                "INSERT OR IGNORE INTO tool_types(id, name, record_json) VALUES (?1, ?2, ?3)",
                params![record.id, record.name, json],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Loads type records alphabetically by their user-visible names.
pub fn list(connection: &Connection) -> Result<Vec<ToolTypeRecord>, String> {
    let mut statement = connection
        .prepare("SELECT record_json FROM tool_types ORDER BY name COLLATE NOCASE, id")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.map(|row| {
        let json = row.map_err(|error| error.to_string())?;
        serde_json::from_str(&json).map_err(|error| format!("corrupt tool type record: {error}"))
    })
    .collect()
}

/// Validates and inserts or updates one tool type, keeping its id stable across renames.
pub fn upsert(connection: &Connection, record: &ToolTypeRecord) -> Result<(), String> {
    validate(record)?;
    let json = serde_json::to_string(record).map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO tool_types(id, name, record_json) VALUES (?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET name=excluded.name, record_json=excluded.record_json",
            params![record.id, record.name, json],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Removes one type by local id and reports whether it existed.
pub fn delete(connection: &Connection, id: &str) -> Result<bool, String> {
    connection
        .execute("DELETE FROM tool_types WHERE id=?1", [id])
        .map(|changed| changed > 0)
        .map_err(|error| error.to_string())
}

/// Enforces identifier, text, cutting-family, and built-in geometry bounds.
pub fn validate(record: &ToolTypeRecord) -> Result<(), String> {
    if record.id.is_empty()
        || record.id.len() > 80
        || !record
            .id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_-".contains(ch))
    {
        return Err("tool type id must be 1-80 ASCII letters, digits, '_' or '-'".into());
    }
    if record.name.trim().is_empty()
        || record.name.trim().len() > 80
        || record.name.chars().any(char::is_control)
    {
        return Err("tool type name must contain 1-80 printable characters".into());
    }
    if !["milling", "drilling", "laser", "probing", "other"].contains(&record.cutting_type.as_str())
    {
        return Err("unsupported tool cutting type".into());
    }
    if ![
        "cylindrical",
        "ball_nose",
        "tapered",
        "v_bit",
        "drill",
        "laser_spot",
        "probe_tip",
        "none",
    ]
    .contains(&record.geometry_model.as_str())
    {
        return Err("unsupported tool geometry model".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates an in-memory database using the same schema and defaults as production.
    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        ensure_schema(&connection).unwrap();
        connection
    }

    /// Confirms default records are idempotent and custom types support complete CRUD.
    #[test]
    fn tool_type_crud_and_default_seeding_are_stable() {
        let db = database();
        ensure_schema(&db).unwrap();
        assert_eq!(list(&db).unwrap().len(), 6);
        let custom = ToolTypeRecord {
            id: "custom-cutter".into(),
            name: "Custom cutter".into(),
            cutting_type: "milling".into(),
            geometry_model: "tapered".into(),
        };
        upsert(&db, &custom).unwrap();
        assert!(list(&db).unwrap().contains(&custom));
        assert!(delete(&db, &custom.id).unwrap());
        assert!(!delete(&db, &custom.id).unwrap());
    }

    /// Rejects unsafe identifiers, unknown cutting families, and arbitrary geometry values.
    #[test]
    fn invalid_tool_types_are_rejected() {
        let mut record = ToolTypeRecord {
            id: "valid".into(),
            name: "Valid".into(),
            cutting_type: "milling".into(),
            geometry_model: "cylindrical".into(),
        };
        record.geometry_model = "uploaded_mesh".into();
        assert!(validate(&record).is_err());
        record.geometry_model = "cylindrical".into();
        record.cutting_type = "unsafe-estimator".into();
        assert!(validate(&record).is_err());
    }
}
