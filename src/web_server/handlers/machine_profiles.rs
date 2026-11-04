//! HTTP endpoints for listing, editing, selecting, importing and exporting local machine profiles.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::web_server::machine_profiles::{
    export_document, MachineProfile, MachineProfileDocument,
};
use crate::web_server::SharedController;

/// Request body for creating or updating one user-authored profile.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MachineProfileRequest {
    /// Profile payload validated by the domain layer before persistence.
    pub(crate) profile: MachineProfile,
}

/// Request body for importing a versioned portable profile document.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MachineProfileImportRequest {
    /// Export envelope with a schema version and one or more profiles.
    pub(crate) document: MachineProfileDocument,
}

/// Request body selecting or clearing the local preflight profile.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MachineProfileSelectionRequest {
    /// Profile id to select, or `None` to clear the selection.
    pub(crate) profile_id: Option<String>,
}

/// Lists all local profiles in deterministic order.
pub(crate) async fn get_machine_profiles(
    State(state): State<SharedController>,
) -> Result<Json<Vec<MachineProfile>>, (StatusCode, String)> {
    state
        .list_machine_profiles()
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Returns a portable versioned JSON envelope suitable for local export.
pub(crate) async fn export_machine_profiles(
    State(state): State<SharedController>,
) -> Result<Json<MachineProfileDocument>, (StatusCode, String)> {
    state
        .list_machine_profiles()
        .map(export_document)
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Returns a read-only profile/controller metadata snapshot with source provenance and conflicts.
pub(crate) async fn get_resolved_machine_configuration(
    State(state): State<SharedController>,
) -> Result<Json<crate::web_server::machine_profiles::ResolvedConfiguration>, (StatusCode, String)>
{
    let profile_id = state
        .active_machine_profile_id()
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    state
        .resolved_machine_configuration(profile_id.as_deref())
        .map(Json)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// Returns the selected profile id without inferring it from a discovery endpoint.
pub(crate) async fn get_active_machine_profile(
    State(state): State<SharedController>,
) -> Result<Json<Option<String>>, (StatusCode, String)> {
    state
        .active_machine_profile_id()
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// Selects a user-authored profile for preflight without applying values to the controller.
pub(crate) async fn select_machine_profile(
    State(state): State<SharedController>,
    Json(request): Json<MachineProfileSelectionRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .select_machine_profile(request.profile_id.as_deref())
        .map(|()| StatusCode::NO_CONTENT)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// Validates and stores one user-authored profile, returning conflict for a duplicate profile name.
pub(crate) async fn put_machine_profile(
    State(state): State<SharedController>,
    Json(request): Json<MachineProfileRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .save_machine_profile(&request.profile)
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

/// Deletes a profile by its local id without affecting machine selection or controller state.
pub(crate) async fn delete_machine_profile(
    State(state): State<SharedController>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    match state.delete_machine_profile(&id) {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err((
            StatusCode::NOT_FOUND,
            "machine profile does not exist".into(),
        )),
        Err(error) => Err((StatusCode::INTERNAL_SERVER_ERROR, error)),
    }
}

/// Imports profiles atomically; invalid documents leave the stored collection unchanged.
pub(crate) async fn import_machine_profiles(
    State(state): State<SharedController>,
    Json(request): Json<MachineProfileImportRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .import_machine_profiles(&request.document)
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
