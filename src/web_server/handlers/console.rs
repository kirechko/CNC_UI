//! Status, machine-list, console-command, log and program endpoints.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::web_server::models::{
    request_language, LangQuery, LoadedProgram, LogPage, LogQuery, StatusResponse,
};
use crate::web_server::program::{load_program, ProgramError, ProgramRequest};
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

/// Overlays normalized LinuxCNC bridge telemetry onto the controller snapshot for UI serialization.
fn apply_linuxcnc_status(
    snapshot: &mut crate::cnc_controller::ControllerSnapshot,
    payload: &serde_json::Value,
) {
    let status = &mut snapshot.machine;
    status.connected = payload
        .get("connected")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if let Some(position) = payload
        .get("machine_pos")
        .and_then(serde_json::Value::as_array)
    {
        let values: Vec<f64> = position
            .iter()
            .filter_map(serde_json::Value::as_f64)
            .collect();
        for (target, value) in [
            (&mut status.position.x, values.first()),
            (&mut status.position.y, values.get(1)),
            (&mut status.position.z, values.get(2)),
            (&mut status.position.a, values.get(3)),
            (&mut status.position.b, values.get(4)),
            (&mut status.position.c, values.get(5)),
        ] {
            if let Some(value) = value {
                *target = *value;
            }
        }
    }
    if let Some(work) = payload
        .get("work_pos")
        .and_then(serde_json::Value::as_array)
    {
        let values: Vec<f64> = work.iter().filter_map(serde_json::Value::as_f64).collect();
        for (target, value) in [
            (&mut status.work_offset.x, values.first()),
            (&mut status.work_offset.y, values.get(1)),
            (&mut status.work_offset.z, values.get(2)),
            (&mut status.work_offset.a, values.get(3)),
            (&mut status.work_offset.b, values.get(4)),
            (&mut status.work_offset.c, values.get(5)),
        ] {
            if let Some(value) = value {
                *target = *value;
            }
        }
    }
    status.feed_rate = payload
        .get("feed_rate")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(status.feed_rate);
    status.homed = payload
        .get("homed")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            std::array::from_fn(|index| {
                items
                    .get(index)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(status.homed);
    status.task_state = if payload.get("estop").and_then(serde_json::Value::as_bool) == Some(true) {
        crate::cnc_api::TaskState::Estop
    } else if payload.get("enabled").and_then(serde_json::Value::as_bool) == Some(true) {
        crate::cnc_api::TaskState::On
    } else {
        crate::cnc_api::TaskState::Off
    };
    status.current_line = payload
        .get("current_line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;
    status.loaded_program = payload
        .get("file")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
}

/// `GET /api/status`: the flat snapshot consumed by `web/js/main.js`, enriched
/// with the latest log messages for the header chip.
pub(crate) async fn get_status(
    State(state): State<SharedController>,
) -> Result<Json<StatusResponse>, (StatusCode, String)> {
    let status = {
        let controller = state.controller.lock().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "controller lock poisoned".into(),
            )
        })?;
        controller.get_status_snapshot()
    };
    let mut status = status;
    // When a bridge is selected, its NML snapshot is authoritative; a failed
    // refresh marks that remote connection down instead of showing emulator data.
    let linuxcnc_endpoint = state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone());
    if let Some(endpoint) = linuxcnc_endpoint.as_ref() {
        match super::super::state::linuxcnc_bridge::request(
            endpoint,
            "status",
            serde_json::json!({}),
            state
                .linuxcnc_request_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::Duration::from_millis(800),
        ) {
            Ok(payload) => apply_linuxcnc_status(&mut status, &payload),
            Err(error) => {
                status.machine.connected = false;
                status.machine.message =
                    Some(format!("LinuxCNC bridge unavailable: {}", error.message));
            }
        }
    }
    let connected_device_id = if let Some(endpoint) = linuxcnc_endpoint {
        Some(format!("linuxcnc:{endpoint}"))
    } else {
        state.connected_machine_id()
    };
    let devices = state.machine_snapshot();
    let mut response =
        StatusResponse::from(status).with_connection_info(&devices, connected_device_id.as_deref());
    if let Ok(latest) = state.latest_log_messages() {
        response.latest_critical = latest.critical;
        response.latest_other = latest.other;
    }
    if std::env::var_os("CNC_UI_HTTP_STATUS_TRACE").is_some()
        && matches!(response.state.as_str(), "READING" | "RUNNING")
    {
        match serde_json::to_string(&response) {
            Ok(payload) => eprintln!("[HTTP STATUS] {payload}"),
            Err(error) => eprintln!("[HTTP STATUS] serialization failed: {error}"),
        }
    }
    Ok(Json(response))
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
        let result = super::super::state::linuxcnc_bridge::request(
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

/// `GET /api/toolpath`: the parsed toolpath of the loaded program.
pub(crate) async fn get_toolpath(
    State(state): State<SharedController>,
) -> Result<Json<cnc_core::Toolpath>, (StatusCode, String)> {
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    Ok(Json(controller.get_status_snapshot().toolpath))
}

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

/// `POST /api/program/run`: starts the loaded program.
pub(crate) async fn start_program(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    if let Some(endpoint) = state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
    {
        // Upload then run ensures the remote interpreter opens exactly the loaded UI program.
        let loaded = state
            .program
            .lock()
            .ok()
            .and_then(|value| value.clone())
            .ok_or_else(|| (StatusCode::BAD_REQUEST, "no program loaded".to_string()))?;
        let remote_name = loaded.name.clone();
        let upload_id = state
            .linuxcnc_request_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tokio::task::spawn_blocking({
            let endpoint = endpoint.clone();
            let content = loaded.content.clone();
            move || {
                super::super::state::linuxcnc_bridge::request(
                    &endpoint,
                    "program_upload",
                    serde_json::json!({"name":remote_name,"content":content}),
                    upload_id,
                    std::time::Duration::from_secs(30),
                )
            }
        })
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.message))?;
        let run_endpoint = endpoint.clone();
        let run_id = state
            .linuxcnc_request_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let run_name = loaded.name;
        tokio::task::spawn_blocking(move || {
            super::super::state::linuxcnc_bridge::request(
                &run_endpoint,
                "program_run",
                serde_json::json!({"name":run_name,"start_line":0}),
                run_id,
                std::time::Duration::from_secs(5),
            )
        })
        .await
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.message))?;
        return Ok(StatusCode::ACCEPTED);
    }
    state
        .start_loaded_program(language)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
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
        super::super::state::linuxcnc_bridge::request(
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
        super::super::state::linuxcnc_bridge::request(
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
        super::super::state::linuxcnc_bridge::request(
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

/// `POST /api/reset-alarm`: forwards a controller alarm reset.
pub(crate) async fn post_reset_alarm(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".to_string(),
        )
    })?;
    controller.reset_alarm();
    Ok(StatusCode::ACCEPTED)
}
