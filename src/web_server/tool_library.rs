//! Local, user-authored cutting-tool records with versioned JSON import and export.
//!
//! Tool records are descriptive metadata only. This module never emits M6 or other machine commands.

use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Current schema version for portable tool-library documents.
pub(crate) const TOOL_DOCUMENT_VERSION: u32 = 1;

/// One local cutting tool entry that may be matched to a G-code T word for operator review.
/// Primitive geometry is a local tool-type descriptor only; arbitrary mesh payload is stored separately.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolRecord {
    /// Stable local identifier that is independent of the controller's currently selected tool.
    pub id: String,
    /// Numeric tool number matched against G-code T words.
    pub number: u32,
    /// Human-readable tool category retained for compatibility with existing tool-library documents.
    pub tool_type: String,
    /// Stable optional reference to the independently stored tool type definition.
    #[serde(default)]
    pub tool_type_id: Option<String>,
    /// Optional cutting diameter in explicitly declared library units.
    #[serde(default)]
    pub diameter: Option<f64>,
    /// Optional gauge or cutting length in the same declared units.
    #[serde(default)]
    pub length: Option<f64>,
    /// Optional material or workpiece note associated with this tool.
    #[serde(default)]
    pub material: Option<String>,
    /// Optional user-entered spindle speed recommendation, not an adapter command.
    #[serde(default)]
    pub rpm: Option<f64>,
    /// Optional user-entered feed recommendation, not an adapter command.
    #[serde(default)]
    pub feed: Option<f64>,
    /// Optional relative or absolute image reference retained as metadata only.
    #[serde(default)]
    pub photo: Option<String>,
    /// Relative filename of an operator-supplied STL/OBJ/Three.js mesh asset, never served as executable content.
    #[serde(default)]
    pub geometry: Option<String>,
    /// Opaque extension object preserved across import/export for forward compatibility.
    #[serde(default)]
    pub extensions: serde_json::Value,
}

/// Versioned export envelope for lossless tool-library transfer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolLibraryDocument {
    /// Schema version needed to interpret the document.
    pub schema_version: u32,
    /// Tool entries contained in this document.
    pub tools: Vec<ToolRecord>,
}

/// Creates the local tool table and its version marker idempotently.
pub fn ensure_schema(connection: &Connection) -> Result<(), String> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS tool_library (
            id TEXT PRIMARY KEY,
            number INTEGER NOT NULL,
            record_json TEXT NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS tool_library_number_unique ON tool_library(number);
         CREATE TABLE IF NOT EXISTS app_schema_migrations (
            name TEXT PRIMARY KEY,
            applied_at_ms INTEGER NOT NULL
         );
         INSERT OR IGNORE INTO app_schema_migrations(name, applied_at_ms) VALUES ('tool_library_v1', 0);"
    ).map_err(|error| format!("cannot initialize tool library schema: {error}"))
}

/// Loads all tool records ordered by tool number, then stable local id.
pub fn list(connection: &Connection) -> Result<Vec<ToolRecord>, String> {
    let mut statement = connection
        .prepare("SELECT record_json FROM tool_library ORDER BY number, id")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.map(|row| {
        let json = row.map_err(|error| error.to_string())?;
        serde_json::from_str(&json).map_err(|error| format!("corrupt tool record: {error}"))
    })
    .collect()
}

/// Inserts or replaces one validated tool record while enforcing one record per T-word number and known type references.
pub fn upsert(connection: &mut Connection, tool: &ToolRecord) -> Result<(), String> {
    validate(tool)?;
    validate_tool_type_reference(connection, tool)?;
    let json = serde_json::to_string(tool).map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO tool_library(id, number, record_json) VALUES (?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET number=excluded.number, record_json=excluded.record_json",
            params![tool.id, tool.number, json],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Removes one tool by stable local id and returns whether it existed.
pub fn delete(connection: &Connection, id: &str) -> Result<bool, String> {
    connection
        .execute("DELETE FROM tool_library WHERE id=?1", [id])
        .map(|changed| changed > 0)
        .map_err(|error| error.to_string())
}

/// Validates every imported entry before atomically upserting the document contents.
pub fn import_document(
    connection: &mut Connection,
    document: &ToolLibraryDocument,
) -> Result<(), String> {
    if document.schema_version != TOOL_DOCUMENT_VERSION {
        return Err(format!(
            "unsupported tool library schema version {}",
            document.schema_version
        ));
    }
    if document.tools.len() > 2000 {
        return Err("tool library document exceeds the 2000 tool limit".into());
    }
    let mut ids = HashSet::new();
    let mut numbers = HashSet::new();
    for tool in &document.tools {
        validate(tool)?;
        validate_tool_type_reference(connection, tool)?;
        if !ids.insert(tool.id.as_str()) || !numbers.insert(tool.number) {
            return Err("tool import contains duplicate id or tool number".into());
        }
    }
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| error.to_string())?;
    for tool in &document.tools {
        let json = serde_json::to_string(tool).map_err(|error| error.to_string())?;
        tx.execute(
            "INSERT INTO tool_library(id, number, record_json) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET number=excluded.number, record_json=excluded.record_json",
            params![tool.id, tool.number, json],
        ).map_err(|error| error.to_string())?;
    }
    tx.commit().map_err(|error| error.to_string())
}

/// Rejects a dangling local tool-type reference while allowing legacy free-text records.
fn validate_tool_type_reference(connection: &Connection, tool: &ToolRecord) -> Result<(), String> {
    let Some(type_id) = tool.tool_type_id.as_deref() else {
        return Ok(());
    };
    let exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM tool_types WHERE id=?1)",
            [type_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if exists {
        Ok(())
    } else {
        Err(format!("tool type `{type_id}` does not exist"))
    }
}

/// Validates bounds and text lengths without interpreting recommendations as machine-enforced limits.
pub fn validate(tool: &ToolRecord) -> Result<(), String> {
    if tool.id.is_empty()
        || tool.id.len() > 80
        || !tool
            .id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_-".contains(ch))
    {
        return Err("tool id must be 1-80 ASCII letters, digits, '_' or '-'".into());
    }
    if tool.number == 0 || tool.number > 999_999 {
        return Err("tool number must be between 1 and 999999".into());
    }
    if tool.tool_type.trim().is_empty() || tool.tool_type.trim().len() > 80 {
        return Err("tool type must contain 1-80 characters".into());
    }
    for (label, value) in [
        ("diameter", tool.diameter),
        ("length", tool.length),
        ("rpm", tool.rpm),
        ("feed", tool.feed),
    ] {
        if value.is_some_and(|number| !number.is_finite() || number < 0.0) {
            return Err(format!("tool {label} must be a finite non-negative number"));
        }
    }
    for (label, value, limit) in [
        ("material", tool.material.as_deref(), 120),
        ("photo", tool.photo.as_deref(), 2048),
        ("geometry", tool.geometry.as_deref(), 2048),
    ] {
        if value.is_some_and(|text| text.len() > limit || text.chars().any(char::is_control)) {
            return Err(format!(
                "tool {label} is too long or contains control characters"
            ));
        }
    }
    if serde_json::to_vec(&tool.extensions)
        .map_err(|error| error.to_string())?
        .len()
        > 65_536
    {
        return Err("tool extensions exceed 65536 bytes".into());
    }
    Ok(())
}

/// Builds a versioned export document from stored tool records.
pub fn export_document(tools: Vec<ToolRecord>) -> ToolLibraryDocument {
    ToolLibraryDocument {
        schema_version: TOOL_DOCUMENT_VERSION,
        tools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates an isolated in-memory database with the production tool-library schema.
    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        ensure_schema(&connection).unwrap();
        crate::web_server::tool_types::ensure_schema(&connection).unwrap();
        connection
    }

    /// Creates a valid sample record with no implicit machine command behavior.
    fn sample() -> ToolRecord {
        ToolRecord {
            id: "tool-2".into(),
            number: 2,
            tool_type: "end mill".into(),
            tool_type_id: None,
            geometry: None,
            diameter: Some(3.175),
            length: None,
            material: Some("aluminum".into()),
            rpm: Some(12000.0),
            feed: None,
            photo: None,
            extensions: serde_json::json!({}),
        }
    }

    /// Confirms CRUD and versioned JSON transfer preserve optional metadata losslessly.
    #[test]
    fn tool_crud_and_json_round_trip() {
        let mut db = database();
        let tool = sample();
        upsert(&mut db, &tool).unwrap();
        let doc = export_document(list(&db).unwrap());
        let decoded: ToolLibraryDocument =
            serde_json::from_str(&serde_json::to_string(&doc).unwrap()).unwrap();
        let mut other = database();
        import_document(&mut other, &decoded).unwrap();
        assert_eq!(list(&other).unwrap(), vec![tool.clone()]);
        assert!(delete(&other, "tool-2").unwrap());
    }

    /// Rejects invalid entries and prevents duplicate tool numbers in one imported document.
    #[test]
    fn invalid_tool_and_duplicate_numbers_are_rejected() {
        let mut invalid = sample();
        invalid.diameter = Some(f64::NAN);
        assert!(validate(&invalid).is_err());
        let mut db = database();
        let mut duplicate = sample();
        duplicate.id = "other".into();
        let document = ToolLibraryDocument {
            schema_version: TOOL_DOCUMENT_VERSION,
            tools: vec![sample(), duplicate],
        };
        assert!(import_document(&mut db, &document)
            .unwrap_err()
            .contains("duplicate"));
        assert!(list(&db).unwrap().is_empty());
    }

    /// Rejects a selected tool category that has been removed instead of persisting a dangling reference.
    #[test]
    fn tool_type_reference_must_exist() {
        let mut db = database();
        let mut tool = sample();
        tool.tool_type_id = Some("missing-type".into());
        assert!(upsert(&mut db, &tool)
            .unwrap_err()
            .contains("does not exist"));
    }
}
