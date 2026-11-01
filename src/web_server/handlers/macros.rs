//! HTTP endpoints for reading and replacing the user-defined macro collection.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;

use crate::web_server::models::{self, MacroDefinition, MacroFolder};
use crate::web_server::SharedController;

/// Optional language parameter used to localize macro validation errors.
#[derive(Deserialize, Default)]
pub(crate) struct MacroLanguageQuery {
    /// Explicit locale override; otherwise `Accept-Language` is used.
    lang: Option<String>,
}

/// Computes the response language using the shared backend negotiation policy.
fn language(query: &MacroLanguageQuery, headers: &HeaderMap) -> &'static str {
    models::request_language(
        &models::LangQuery {
            lang: query.lang.clone(),
        },
        headers,
    )
}

/// JSON wrapper matching the desktop command argument shape for macro replacement.
#[derive(Deserialize)]
pub(crate) struct MacroListRequest {
    /// Complete desired macro collection.
    macros: Vec<MacroDefinition>,
}

/// Returns the complete macro list.
pub(crate) async fn get_macros(
    State(state): State<SharedController>,
) -> Result<Json<Vec<MacroDefinition>>, (StatusCode, String)> {
    state
        .list_macros()
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Returns the complete folder list used by the macro tree navigator.
pub(crate) async fn get_macro_folders(
    State(state): State<SharedController>,
) -> Result<Json<Vec<MacroFolder>>, (StatusCode, String)> {
    state
        .list_macro_folders()
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Body for creating a macro folder under the root or another folder.
#[derive(Deserialize)]
pub(crate) struct MacroFolderRequest {
    folder: MacroFolder,
}

/// Creates one folder after validating its name and parent relationship.
pub(crate) async fn post_macro_folder(
    State(state): State<SharedController>,
    Query(query): Query<MacroLanguageQuery>,
    headers: HeaderMap,
    Json(request): Json<MacroFolderRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let locale = language(&query, &headers);
    models::validate_macro_folder(&request.folder, locale)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    state
        .create_macro_folder(&request.folder)
        .map(|()| StatusCode::CREATED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// Atomically replaces the complete macro list after validating its content and metadata.
pub(crate) async fn put_macros(
    State(state): State<SharedController>,
    Query(query): Query<MacroLanguageQuery>,
    headers: HeaderMap,
    Json(request): Json<MacroListRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = language(&query, &headers);
    models::validate_macros(&request.macros, language)
        .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    state
        .replace_macros(&request.macros)
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Deletes an empty macro folder without deleting nested user content.
pub(crate) async fn delete_macro_folder(
    State(state): State<SharedController>,
    axum::extract::Path(folder_id): axum::extract::Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .delete_macro_folder(&folder_id)
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| {
            let status = if error.contains("not empty") {
                StatusCode::CONFLICT
            } else {
                StatusCode::NOT_FOUND
            };
            (status, error)
        })
}
