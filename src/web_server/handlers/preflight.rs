//! HTTP endpoint for source-level preflight analysis.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::web_server::state::preflight::PreflightReport;
use crate::web_server::SharedController;

/// Request body selecting the G-code source and optional local profile for preflight.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreflightRequest {
    /// Explicit source text; omitted to analyze the currently loaded program.
    pub(crate) content: Option<String>,
    /// Local profile id; absent selects the profile explicitly stored in application state.
    pub(crate) profile_id: Option<String>,
}

/// Returns static Rosetta-based findings and never starts, pauses, or changes a machine.
pub(crate) async fn preflight_program(
    State(state): State<SharedController>,
    Json(request): Json<PreflightRequest>,
) -> Result<Json<PreflightReport>, (StatusCode, String)> {
    let source = request
        .content
        .or_else(|| state.loaded_program_text())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "no program content supplied or loaded".to_string(),
            )
        })?;
    let profile_id = match request.profile_id {
        Some(id) => Some(id),
        None => state
            .active_machine_profile_id()
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?,
    };
    state
        .preflight_program(&source, profile_id.as_deref())
        .map(Json)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}
