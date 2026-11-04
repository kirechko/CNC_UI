//! Application-state accessors for independently stored tool types.

use super::AppState;
use crate::web_server::tool_types::{self, ToolTypeRecord};

impl AppState {
    /// Lists user-editable type definitions from SQLite without machine interaction.
    pub(crate) fn list_tool_types(&self) -> Result<Vec<ToolTypeRecord>, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "tool database lock poisoned".to_string())?;
        tool_types::list(&connection)
    }

    /// Validates and saves one independent tool type record.
    pub(crate) fn save_tool_type(&self, record: &ToolTypeRecord) -> Result<(), String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "tool database lock poisoned".to_string())?;
        tool_types::upsert(&connection, record)
    }

    /// Deletes one type definition by id and returns whether it existed.
    pub(crate) fn delete_tool_type(&self, id: &str) -> Result<bool, String> {
        let mut connection = self
            .log
            .lock()
            .map_err(|_| "tool database lock poisoned".to_string())?;
        let tx = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        if crate::web_server::tool_library::list(&tx)?
            .iter()
            .any(|tool| tool.tool_type_id.as_deref() == Some(id))
        {
            return Err("tool type is referenced by a tool record".into());
        }
        let deleted = tool_types::delete(&tx, id)?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(deleted)
    }
}
