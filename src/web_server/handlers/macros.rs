//! HTTP endpoints for reading and replacing the user-defined macro collection.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;

use crate::web_server::models::{self, MacroDefinition};
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
