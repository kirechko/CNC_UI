use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::cnc_api::Axis;
use crate::web_server::models::axis_letter;
use crate::web_server::SharedController;

use super::spindle::bridge_command;
/// Body of `POST /api/jog` (single-axis incremental step).
#[derive(Deserialize)]
pub(crate) struct JogRequest {
    axis: Axis,
    direction: i8,
    distance: f64,
}

/// Body of `POST /api/jog/xy` (diagonal XY step; camelCase wire names).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct XyJogRequest {
    // Fields are `pub(crate)` so the JSON contract tests can inspect the
    // deserialized camelCase mapping from outside this module.
    pub(crate) x_direction: i8,
    pub(crate) y_direction: i8,
    pub(crate) distance: f64,
}

/// Body of `POST /api/jog/continuous` (axis hold-to-move).
#[derive(Deserialize)]
pub(crate) struct ContinuousJogRequest {
    axis: Axis,
    direction: i8,
}

/// Body of `POST /api/jog/continuous-xy` (XY hold-to-move).
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContinuousXyJogRequest {
    // See `XyJogRequest`: the contract tests assert the camelCase wire names.
    pub(crate) x_direction: i8,
    pub(crate) y_direction: i8,
}

/// Body of `POST /api/jog/speed`. Each field is optional so a single group
/// can be updated. Values are feed rates in mm/min (rotary axes use the same
/// unit as configured for the machine).
///
/// `rotary` is the legacy shared A–C field and stays supported; the per-axis
/// `a`/`b`/`c` fields (task: per-axis jog speed controls) set individual
/// rotary feeds. When both a `rotary` and per-axis value are present, the
/// per-axis value wins for that axis.
#[derive(Deserialize)]
pub(crate) struct JogSpeedRequest {
    xy: Option<f64>,
    z: Option<f64>,
    rotary: Option<f64>,
    a: Option<f64>,
    b: Option<f64>,
    c: Option<f64>,
}

/// `POST /api/jog`: one incremental step along a single axis.
pub(crate) async fn jog(
    State(state): State<SharedController>,
    Json(request): Json<JogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if ![-1, 1].contains(&request.direction)
        || !request.distance.is_finite()
        || request.distance <= 0.0
        || request.distance > 100_000.0
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "invalid jog direction or distance".into(),
        ));
    }
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        // Map the UI axis enum to LinuxCNC's INI coordinate ordering.
        let axis_index = match request.axis {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
            Axis::A => 3,
            Axis::B => 4,
            Axis::C => 5,
        };
        bridge_command(
            &state,
            "jog_increment",
            serde_json::json!({"axis":axis_index,"distance":request.distance * f64::from(request.direction),"velocity":120.0}),
            std::time::Duration::from_secs(5),
        ).await?;
        return Ok(StatusCode::ACCEPTED);
    }
    let axis = axis_letter(request.axis);
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller.jog_axis_step(axis, request.direction, request.distance);
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/jog/xy`: one incremental diagonal XY step.
pub(crate) async fn jog_xy(
    State(state): State<SharedController>,
    Json(request): Json<XyJogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let linuxcnc_endpoint = state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone());
    if linuxcnc_endpoint.is_some() {
        if ![-1, 1].contains(&request.x_direction)
            || ![-1, 1].contains(&request.y_direction)
            || !request.distance.is_finite()
            || request.distance <= 0.0
            || request.distance > 100_000.0
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "invalid XY jog direction or distance".into(),
            ));
        }
        let distance = request.distance;
        bridge_command(&state, "mdi", serde_json::json!({"block":format!("G91 G0 X{} Y{}\nG90", request.x_direction as f64 * distance, request.y_direction as f64 * distance)}), std::time::Duration::from_secs(10)).await?;
        return Ok(StatusCode::ACCEPTED);
    }
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller.jog_xy_step(request.x_direction, request.y_direction, request.distance);
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/jog/continuous`: start (direction ±1) or stop (0) a hold-to-move
/// jog on one axis.
pub(crate) async fn jog_continuous(
    State(state): State<SharedController>,
    Json(request): Json<ContinuousJogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        let axis = match request.axis {
            Axis::X => 0,
            Axis::Y => 1,
            Axis::Z => 2,
            Axis::A => 3,
            Axis::B => 4,
            Axis::C => 5,
        };
        let command = if request.direction == 0 {
            "jog_stop"
        } else {
            "jog_start"
        };
        bridge_command(
            &state,
            command,
            serde_json::json!({"axis":axis,"velocity":request.direction as f64 * 100.0}),
            std::time::Duration::from_secs(3),
        )
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
    if ![-1, 0, 1].contains(&request.direction) {
        return Err((
            StatusCode::BAD_REQUEST,
            "direction must be -1, 0, or 1".into(),
        ));
    }

    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    let axis = axis_letter(request.axis);
    controller.jog_axis_continuous(axis, request.direction);
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/jog/continuous-xy`: hold-to-move on the XY plane.
pub(crate) async fn jog_continuous_xy(
    State(state): State<SharedController>,
    Json(request): Json<ContinuousXyJogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        if ![-1, 0, 1].contains(&request.x_direction) || ![-1, 0, 1].contains(&request.y_direction)
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "XY jog directions must be -1, 0, or 1".into(),
            ));
        }
        // Axis stop requests are idempotent; iterating both coordinates safely releases diagonal holds.
        for (axis, direction) in [(0, request.x_direction), (1, request.y_direction)] {
            let command = if direction == 0 {
                "jog_stop"
            } else {
                "jog_start"
            };
            bridge_command(
                &state,
                command,
                serde_json::json!({"axis":axis,"velocity":direction as f64 * 100.0}),
                std::time::Duration::from_secs(3),
            )
            .await?;
        }
        return Ok(StatusCode::ACCEPTED);
    }
    if ![-1, 0, 1].contains(&request.x_direction) || ![-1, 0, 1].contains(&request.y_direction) {
        return Err((
            StatusCode::BAD_REQUEST,
            "XY directions must be -1, 0, or 1".into(),
        ));
    }

    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller.jog_xy_continuous(request.x_direction, request.y_direction);
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/jog/cancel`: immediately stops any active jog motion.
pub(crate) async fn jog_cancel(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        for axis in 0..6 {
            let _ = bridge_command(
                &state,
                "jog_stop",
                serde_json::json!({"axis":axis}),
                std::time::Duration::from_secs(2),
            )
            .await?;
        }
        return Ok(StatusCode::ACCEPTED);
    }
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller.cancel_jog();
    Ok(StatusCode::ACCEPTED)
}

/// `POST /api/jog/speed`: stores the XY, Z and rotary (A–C) jog feeds.
/// Rejects the request if any provided value is not finite and positive.
pub(crate) async fn jog_speed(
    State(state): State<SharedController>,
    Json(request): Json<JogSpeedRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let values = [
        request.xy,
        request.z,
        request.rotary,
        request.a,
        request.b,
        request.c,
    ];
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
        && values.iter().any(Option::is_some)
    {
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            "per-axis jog speed overrides are not supported by the LinuxCNC bridge yet".into(),
        ));
    }
    if values
        .iter()
        .flatten()
        .any(|value| !value.is_finite() || *value <= 0.0 || *value > 100_000.0)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "jog speeds must be finite, greater than 0 and at most 100000".into(),
        ));
    }

    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
        && values.iter().any(Option::is_some)
    {
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            "LinuxCNC bridge jog-speed settings are not implemented".into(),
        ));
    }
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    if let Some(value) = request.xy {
        controller.set_xy_jog_speed(value);
    }
    if let Some(value) = request.z {
        controller.set_z_jog_speed(value);
    }
    // Legacy group value applies to every rotary axis first; individual
    // per-axis values (task) then override their own axis.
    if let Some(value) = request.rotary {
        controller.set_rotary_jog_speed(value);
    }
    for (axis, value) in [
        (crate::cnc_api::Axis::A, request.a),
        (crate::cnc_api::Axis::B, request.b),
        (crate::cnc_api::Axis::C, request.c),
    ] {
        if let Some(value) = value {
            controller.set_axis_jog_speed(axis, value);
        }
    }
    Ok(StatusCode::OK)
}
