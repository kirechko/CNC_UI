//! Console command, command-history, and message-log endpoints.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::web_server::models::{request_language, LangQuery, LogPage, LogQuery};
use crate::web_server::SharedController;

/// Body of `POST /api/command`.
#[derive(Deserialize)]
pub(crate) struct MachineCommandRequest {
    pub(crate) command: String,
}

/// Response of `POST /api/command`.
#[derive(Serialize)]
pub(crate) struct MachineCommandResponse {
    output: String,
}

/// Body of `POST /api/log`.
#[derive(Deserialize)]
pub(crate) struct LogMessageRequest {
    severity: String,
    source: String,
    message: String,
}
/// `POST /api/command`: forwards one console command to the active controller or LinuxCNC bridge.
pub(crate) async fn post_machine_command(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<MachineCommandRequest>,
) -> Result<Json<MachineCommandResponse>, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    let command = request.command.trim();
    if command.is_empty()
        || command.len() > 4096
        || command.contains('\n')
        || command.contains('\r')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            crate::i18n::translate(language, "api.error.commandLength", &[]),
        ));
    }

    if let Some(endpoint) = state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
    {
        let result = super::super::super::state::linuxcnc_bridge::request(
            &endpoint,
            "mdi",
            serde_json::json!({ "block": command }),
            state
                .linuxcnc_request_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::Duration::from_secs(30),
        )
        .map_err(|error| {
            (
                StatusCode::BAD_GATEWAY,
                format!("{}: {}", error.code, error.message),
            )
        })?;
        return Ok(Json(MachineCommandResponse {
            output: result.to_string(),
        }));
    }
    state
        .execute_command(command)
        .map(|output| Json(MachineCommandResponse { output }))
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// `POST /api/command/cancel`: requests cancellation of the long-running
/// machine command (program upload) and a motion stop.
pub(crate) async fn cancel_machine_command(State(state): State<SharedController>) -> StatusCode {
    state.cancel_machine_command();
    StatusCode::ACCEPTED
}

/// `GET /api/command-history`: the last 500 machine commands for the console
/// up/down arrows.
pub(crate) async fn get_command_history(
    State(state): State<SharedController>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    state
        .command_history()
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// `GET /api/log`: filtered, sorted, paginated message-log records.
pub(crate) async fn get_log(
    State(state): State<SharedController>,
    Query(query): Query<LogQuery>,
) -> Result<Json<LogPage>, (StatusCode, String)> {
    state
        .query_log(&query)
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

/// `POST /api/log`: stores a UI-side message (e.g. console history) into the
/// shared SQLite log.
pub(crate) async fn post_log_message(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<LogMessageRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    let severity = request.severity.trim();
    let source = request.source.trim();
    let message = request.message.trim();

    if !crate::message_log::ALL_SEVERITIES.contains(&severity) {
        return Err((
            StatusCode::BAD_REQUEST,
            crate::i18n::translate(language, "api.error.unknownSeverity", &[]),
        ));
    }
    if source.is_empty() || source.len() > 128 || message.is_empty() || message.len() > 16_384 {
        return Err((
            StatusCode::BAD_REQUEST,
            crate::i18n::translate(language, "api.error.invalidSourceMessage", &[]),
        ));
    }

    state.insert_log(severity, source, message);
    Ok(StatusCode::CREATED)
}
