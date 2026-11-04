//! HTTP endpoints for managing the local tool registry.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::web_server::tool_library::{export_document, ToolLibraryDocument, ToolRecord};
use crate::web_server::SharedController;

/// Request wrapper for saving one local tool record.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolRecordRequest {
    /// Tool metadata to validate and persist.
    pub(crate) tool: ToolRecord,
}

/// Request wrapper for importing a versioned portable tool document.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolImportRequest {
    /// Versioned batch of tool entries.
    pub(crate) document: ToolLibraryDocument,
}

/// Returns locally stored tools in ascending numeric tool order.
pub(crate) async fn get_tools(
    State(state): State<SharedController>,
) -> Result<Json<Vec<ToolRecord>>, (StatusCode, String)> {
    state
        .list_tools()
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Exports the current tool library in a versioned JSON envelope.
pub(crate) async fn export_tools(
    State(state): State<SharedController>,
) -> Result<Json<ToolLibraryDocument>, (StatusCode, String)> {
    state
        .list_tools()
        .map(export_document)
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Creates or updates one tool record after validation.
pub(crate) async fn put_tool(
    State(state): State<SharedController>,
    Json(request): Json<ToolRecordRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .save_tool(&request.tool)
        .map(|()| StatusCode::CREATED)
        .map_err(|error| {
            let status = if error.contains("UNIQUE constraint failed") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, error)
        })
}

/// Deletes one local tool record by id without sending a tool-change command.
pub(crate) async fn delete_tool(
    State(state): State<SharedController>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    match state.delete_tool(&id) {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((StatusCode::NOT_FOUND, "tool does not exist".into())),
        Err(error) => Err((StatusCode::INTERNAL_SERVER_ERROR, error)),
    }
}

/// Imports a tool document atomically after validating schema and duplicate identities.
pub(crate) async fn import_tools(
    State(state): State<SharedController>,
    Json(request): Json<ToolImportRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .import_tools(&request.document)
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            let status = if error.contains("UNIQUE constraint failed") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, error)
        })
}
