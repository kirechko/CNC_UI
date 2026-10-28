//! Realtime control endpoints: spindle, jog (step/continuous/cancel/speed)
//! and position (coordinate system, zero, home, set/go work values).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::cnc_api::Axis;
use crate::web_server::models::{axis_letter, request_language, LangQuery};
use crate::web_server::SharedController;

/// Body of `POST /api/spindle`.
#[derive(Deserialize)]
pub(crate) struct SpindleRequest {
    rpm: f64,
    enabled: bool,
    direction: String,
}

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
#[derive(Deserialize)]
pub(crate) struct JogSpeedRequest {
    xy: Option<f64>,
    z: Option<f64>,
    rotary: Option<f64>,
}

/// Body of `POST /api/position/select` and `POST /api/position/zero`.
#[derive(Deserialize)]
pub(crate) struct CoordinateRequest {
    system: String,
}

/// Body of `POST /api/position/zero-axis` and `POST /api/position/home`.
#[derive(Deserialize)]
pub(crate) struct AxisRequest {
    axis: Axis,
}

/// Body of `POST /api/position/set` and `POST /api/position/go`.
#[derive(Deserialize)]
pub(crate) struct AxisPositionRequest {
    axis: Axis,
    system: String,
    value: f64,
}

/// `POST /api/spindle`: validates the requested RPM against the machine
/// configuration before enabling the spindle.
pub(crate) async fn set_spindle(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<SpindleRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;

    if !request.enabled {
        controller.stop_spindle();
        return Ok(StatusCode::ACCEPTED);
    }

    if !request.rpm.is_finite() {
        return Err((
            StatusCode::BAD_REQUEST,
            crate::i18n::translate(language, "api.error.spindleSpeedFinite", &[]),
        ));
    }

    let status = controller.get_status_snapshot();
    if !status.machine.connected {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            crate::i18n::translate(language, "api.error.noConnection", &[]),
        ));
    }

    let spindle = status
        .machine_configuration
        .as_ref()
        .map(|configuration| &configuration.spindle)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                crate::i18n::translate(language, "api.error.spindleConfigUnavailable", &[]),
            )
        })?;
    let minimum = spindle.min_rpm.unwrap_or(0.0);
    let maximum = spindle.max_rpm.unwrap_or(status.machine.max_spindle_rpm);

    if maximum <= 0.0 || request.rpm < minimum || request.rpm > maximum {
        return Err((
            StatusCode::BAD_REQUEST,
            crate::i18n::translate(
                language,
                "api.error.spindleRange",
                &[
                    ("minimum", format!("{minimum:.0}")),
                    ("maximum", format!("{maximum:.0}")),
                ],
            ),
        ));
    }

    let direction = match request.direction.as_str() {
        "CW" => crate::cnc_controller::SpindleDirection::CW,
        "CCW" => crate::cnc_controller::SpindleDirection::Ccw,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                crate::i18n::translate(language, "api.error.spindleDirection", &[]),
            ));
        }
    };

    controller.set_spindle_speed(request.rpm);
    controller.set_spindle_direction(direction);
    Ok(StatusCode::ACCEPTED)
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
            "direction must be -1 or 1; distance must be greater than 0 and at most 100000 mm"
                .into(),
        ));
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
    if ![-1, 1].contains(&request.x_direction)
        || ![-1, 1].contains(&request.y_direction)
        || !request.distance.is_finite()
        || request.distance <= 0.0
        || request.distance > 100_000.0
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "XY directions must be -1 or 1; distance must be greater than 0 and at most 100000 mm"
                .into(),
        ));
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
    let values = [request.xy, request.z, request.rotary];
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
    if let Some(value) = request.rotary {
        controller.set_rotary_jog_speed(value);
    }
    Ok(StatusCode::OK)
}

/// `POST /api/position/select`: activates a coordinate system (G54–G59.3).
pub(crate) async fn select_coordinate_system(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<CoordinateRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    state
        .select_coordinate_system(&request.system, language)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// `POST /api/position/zero`: sets the work zero of all axes.
pub(crate) async fn zero_all(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<CoordinateRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    state
        .zero_all(&request.system, language)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// `POST /api/position/zero-axis`: zeroes a single axis.
pub(crate) async fn zero_axis(
    State(state): State<SharedController>,
    Json(request): Json<AxisRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .zero_axis(request.axis)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// `POST /api/position/home`: homes a single axis.
pub(crate) async fn home_axis(
    State(state): State<SharedController>,
    Json(request): Json<AxisRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .home_axis(request.axis)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// `POST /api/position/home-all`: homes every axis.
pub(crate) async fn home_all(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .home_all()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// `POST /api/position/set`: writes a work coordinate for one axis (G10).
pub(crate) async fn set_axis_work_value(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<AxisPositionRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    state
        .set_axis_work_value(request.axis, &request.system, request.value, language)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// `POST /api/position/go`: moves one axis to a work coordinate.
pub(crate) async fn go_to_axis_work_value(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<AxisPositionRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    state
        .go_to_axis_work_value(request.axis, &request.system, request.value, language)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}
