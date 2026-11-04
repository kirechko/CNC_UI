//! Status snapshot and toolpath endpoints, including LinuxCNC telemetry overlays.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use crate::web_server::models::StatusResponse;
use crate::web_server::SharedController;

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
        match super::super::super::state::linuxcnc_bridge::request(
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
    state.record_status(&status);
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
