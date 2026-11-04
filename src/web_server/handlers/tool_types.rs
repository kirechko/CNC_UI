//! HTTP handlers for CRUD operations on independent tool type definitions.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::web_server::tool_types::ToolTypeRecord;
use crate::web_server::SharedController;

/// JSON request wrapper used by the tool type create/update endpoint.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolTypeRequest {
    /// Type record to validate and persist.
    pub(crate) tool_type: ToolTypeRecord,
}

/// Lists all configured tool type definitions.
pub(crate) async fn get_tool_types(
    State(state): State<SharedController>,
) -> Result<Json<Vec<ToolTypeRecord>>, (StatusCode, String)> {
    state
        .list_tool_types()
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Creates or updates one user-authored tool type.
pub(crate) async fn put_tool_type(
    State(state): State<SharedController>,
    Json(request): Json<ToolTypeRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .save_tool_type(&request.tool_type)
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

/// Deletes one type definition by stable local id.
pub(crate) async fn delete_tool_type(
    State(state): State<SharedController>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    match state.delete_tool_type(&id) {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((StatusCode::NOT_FOUND, "tool type does not exist".into())),
        Err(error) => Err((StatusCode::INTERNAL_SERVER_ERROR, error)),
    }
}
