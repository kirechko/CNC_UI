//! Application-state operations for the local tool registry.

use super::AppState;
use crate::web_server::tool_library::{self, ToolLibraryDocument, ToolRecord};

impl AppState {
    /// Lists tools from the local registry without issuing tool-change commands.
    pub(crate) fn list_tools(&self) -> Result<Vec<ToolRecord>, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "tool database lock poisoned".to_string())?;
        tool_library::list(&connection)
    }

    /// Saves a validated user-authored tool record without touching the active controller.
    pub(crate) fn save_tool(&self, tool: &ToolRecord) -> Result<(), String> {
        let mut connection = self
            .log
            .lock()
            .map_err(|_| "tool database lock poisoned".to_string())?;
        tool_library::upsert(&mut connection, tool)
    }

    /// Removes one tool record by its local id and reports whether it existed.
    pub(crate) fn delete_tool(&self, id: &str) -> Result<bool, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "tool database lock poisoned".to_string())?;
        tool_library::delete(&connection, id)
    }

    /// Imports a versioned tool document transactionally after validating every record.
    pub(crate) fn import_tools(&self, document: &ToolLibraryDocument) -> Result<(), String> {
        let mut connection = self
            .log
            .lock()
            .map_err(|_| "tool database lock poisoned".to_string())?;
        tool_library::import_document(&mut connection, document)
    }
}
