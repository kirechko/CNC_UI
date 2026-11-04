//! Loaded-program retrieval, upload, pause, resume, and abort endpoints.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;

use crate::web_server::models::{request_language, LangQuery, LoadedProgram};
use crate::web_server::program::{load_program, ProgramError, ProgramRequest};
use crate::web_server::SharedController;

/// `GET /api/program`: the currently loaded program, if any.
pub(crate) async fn get_program(
    State(state): State<SharedController>,
) -> Result<Json<Option<LoadedProgram>>, (StatusCode, String)> {
    let program = state.program.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "program lock poisoned".into(),
        )
    })?;
    Ok(Json(program.clone()))
}

/// `POST /api/program`: uploads (validates + loads) a G-code program.
/// Server-side failures map to 500, client errors to 400.
pub(crate) async fn post_program(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<ProgramRequest>,
) -> Result<Json<LoadedProgram>, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    load_program(&state, &request.file_name, &request.content, language)
        .map(Json)
        .map_err(|error: ProgramError| {
            let status = if error.internal {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, error.message)
        })
}
/// `POST /api/program/pause`.
pub(crate) async fn pause_program(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(endpoint) = state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
    {
        super::super::super::state::linuxcnc_bridge::request(
            &endpoint,
            "program_pause",
            serde_json::json!({}),
            state
                .linuxcnc_request_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::Duration::from_secs(5),
        )
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                format!("{}: {}", error.code, error.message),
            )
        })?;
        return Ok(StatusCode::ACCEPTED);
    }
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller
        .program_pause()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                format!("{}: {}", error.code, error.message),
            )
        })
}

/// `POST /api/program/resume`.
pub(crate) async fn resume_program(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(endpoint) = state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
    {
        super::super::super::state::linuxcnc_bridge::request(
            &endpoint,
            "program_resume",
            serde_json::json!({}),
            state
                .linuxcnc_request_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::Duration::from_secs(5),
        )
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                format!("{}: {}", error.code, error.message),
            )
        })?;
        return Ok(StatusCode::ACCEPTED);
    }
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller
        .program_resume()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                format!("{}: {}", error.code, error.message),
            )
        })
}

/// `POST /api/program/abort`.
pub(crate) async fn abort_program(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(endpoint) = state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
    {
        super::super::super::state::linuxcnc_bridge::request(
            &endpoint,
            "program_abort",
            serde_json::json!({}),
            state
                .linuxcnc_request_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::Duration::from_secs(5),
        )
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                format!("{}: {}", error.code, error.message),
            )
        })?;
        return Ok(StatusCode::ACCEPTED);
    }
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller
        .program_abort()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                format!("{}: {}", error.code, error.message),
            )
        })
}
