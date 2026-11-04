//! Preflight review and confirmed program-run endpoints.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use crate::web_server::models::{request_language, LangQuery};
use crate::web_server::SharedController;

/// Confirmation envelope bound to the exact static preflight result shown to the operator.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunConfirmationRequest {
    /// Fingerprint returned by the immediately preceding preflight request.
    pub(crate) fingerprint: String,
    /// Must be explicitly true; omission cannot implicitly approve warnings.
    pub(crate) confirmed_review: bool,
}
/// `POST /api/program/run`: returns fresh preflight findings; does not start motion.
pub(crate) async fn start_program(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    let _language = request_language(&lang, &headers);
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        return Err((
            StatusCode::PRECONDITION_REQUIRED,
            "LinuxCNC bridge does not implement the confirmed preflight handshake; Run is disabled"
                .into(),
        ));
    }
    // Analyze before invoking the controller so the UI can display findings and request
    // a fresh explicit confirmation when static data is incomplete.
    let loaded = state
        .program
        .lock()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "program lock poisoned".into(),
            )
        })?
        .clone()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "no program loaded".into()))?;
    let profile_id = state
        .active_machine_profile_id()
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let report = state
        .preflight_program(&loaded.content, profile_id.as_deref())
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    Ok(Json(report).into_response().status())
}

/// `POST /api/program/run-confirmed`: starts only after a client confirms a fresh preflight review token.
pub(crate) async fn start_program_confirmed(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<RunConfirmationRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    let loaded = state
        .program
        .lock()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "program lock poisoned".into(),
            )
        })?
        .clone()
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "no program loaded".into()))?;
    let profile_id = state
        .active_machine_profile_id()
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let report = state
        .preflight_program(&loaded.content, profile_id.as_deref())
        .map_err(|error| (StatusCode::BAD_REQUEST, error))?;
    if report.blocked {
        return Err((
            StatusCode::CONFLICT,
            "preflight found a known hard blocker; Run remains disabled".into(),
        ));
    }
    if report.findings.iter().any(|finding| {
        matches!(
            finding.severity,
            crate::web_server::PreflightSeverity::Warning
        )
    }) {
        let linuxcnc_endpoint = state
            .linuxcnc_endpoint
            .lock()
            .ok()
            .and_then(|value| value.clone());
        if linuxcnc_endpoint.is_some() {
            return Err((StatusCode::PRECONDITION_REQUIRED, "the LinuxCNC bridge does not implement the confirmed-preflight handshake; run is disabled".into()));
        }
    }
    let current_fingerprint = report.fingerprint.clone();
    if !request.confirmed_review || request.fingerprint != current_fingerprint {
        return Err((
            StatusCode::PRECONDITION_REQUIRED,
            "preflight report changed or was not confirmed; review the current report before Run"
                .into(),
        ));
    }
    state
        .start_loaded_program(language, true)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}
