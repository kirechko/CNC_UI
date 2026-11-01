//! Realtime control endpoints: spindle, jog (step/continuous/cancel/speed)
//! and position (coordinate system, zero, home, set/go work values).

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::cnc_api::Axis;
use crate::web_server::models::{axis_letter, request_language, LangQuery};
use crate::web_server::SharedController;

/// Body of `POST /api/spindle`; the legacy `rpm` field remains accepted for compatibility.
#[derive(Deserialize)]
pub(crate) struct SpindleRequest {
    #[serde(default)]
    rpm: Option<f64>,
    #[serde(default)]
    power: Option<f64>,
    enabled: bool,
    #[serde(default = "default_spindle_direction")]
    direction: String,
    #[serde(default)]
    mode: Option<String>,
}

/// Supplies clockwise direction when older clients omit a direction for a laser request.
fn default_spindle_direction() -> String {
    "CW".into()
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
    // The bridge is separate from the emulated controller worker, so route its
    // laser endpoint before taking the normal controller lock.
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        let enabled = request.enabled;
        if enabled {
            if let Some(mode) = request.mode.as_deref() {
                bridge_command(
                    &state,
                    "set_laser_mode",
                    serde_json::json!({"mode":mode}),
                    std::time::Duration::from_secs(2),
                )
                .await?;
            }
        }
        let power = if enabled {
            request.power.or(request.rpm).unwrap_or(0.0)
        } else {
            0.0
        };
        if enabled && (!power.is_finite() || !(0.0..=1.0).contains(&power)) {
            return Err((
                StatusCode::BAD_REQUEST,
                "bridge laser power must be normalized in 0..1".into(),
            ));
        }
        if enabled && request.direction.eq_ignore_ascii_case("CCW") {
            return Err((
                StatusCode::BAD_REQUEST,
                "laser output does not support reverse direction".into(),
            ));
        }
        bridge_command(
            &state,
            "set_laser",
            serde_json::json!({"enabled":enabled,"power":power}),
            std::time::Duration::from_secs(2),
        )
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
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

    let status = controller.get_status_snapshot();
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
    let is_laser = spindle.mode != crate::cnc_api::SpindleMode::Standard;
    if request.mode.is_some() && !is_laser {
        return Err((
            StatusCode::BAD_REQUEST,
            "laser mode is not available on a standard spindle".into(),
        ));
    }
    let value = if is_laser {
        request.power.or(request.rpm)
    } else {
        request.rpm.or(request.power)
    }
    .ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "power or rpm value is required".into(),
        )
    })?;

    if !value.is_finite() {
        return Err((
            StatusCode::BAD_REQUEST,
            crate::i18n::translate(language, "api.error.spindleSpeedFinite", &[]),
        ));
    }

    if !status.machine.connected {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            crate::i18n::translate(language, "api.error.noConnection", &[]),
        ));
    }

    let (minimum, maximum) = if is_laser {
        let laser = spindle.laser.as_ref().ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "laser configuration is unavailable".into(),
            )
        })?;
        (laser.min_power, laser.max_power)
    } else {
        (
            spindle.min_rpm.unwrap_or(0.0),
            spindle.max_rpm.unwrap_or(status.machine.max_spindle_rpm),
        )
    };

    if maximum <= 0.0 || value < minimum || value > maximum {
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

    if is_laser && request.direction.eq_ignore_ascii_case("CCW") {
        return Err((
            StatusCode::BAD_REQUEST,
            "laser output does not support reverse direction".into(),
        ));
    }
    controller.set_spindle_speed(value);
    controller.set_spindle_direction(direction);
    Ok(StatusCode::ACCEPTED)
}

/// Selects a supported constant or dynamic laser mode without enabling emission.
/// The controller API owns mode changes so unsupported hardware remains fail-closed.
pub(crate) async fn set_laser_mode(
    State(state): State<SharedController>,
    Json(request): Json<LaserModeRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if let Some(endpoint) = state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
    {
        if !matches!(request.mode.as_str(), "constant" | "dynamic") {
            return Err((
                StatusCode::BAD_REQUEST,
                "mode must be constant or dynamic".into(),
            ));
        }
        super::super::state::linuxcnc_bridge::request(
            &endpoint,
            "set_laser_mode",
            serde_json::json!({ "mode": request.mode }),
            state
                .linuxcnc_request_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::Duration::from_secs(2),
        )
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.message))?;
        return Ok(StatusCode::ACCEPTED);
    }
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    let status = controller.get_status_snapshot();
    let spindle = status
        .machine_configuration
        .as_ref()
        .map(|configuration| &configuration.spindle)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "machine output configuration unavailable".into(),
            )
        })?;
    let desired = match request.mode.as_str() {
        "constant" => crate::cnc_api::SpindleMode::LaserConstant,
        "dynamic" => crate::cnc_api::SpindleMode::LaserDynamic,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "mode must be constant or dynamic".into(),
            ))
        }
    };
    if spindle.mode == crate::cnc_api::SpindleMode::Standard
        || spindle.laser.is_none()
        || (desired == crate::cnc_api::SpindleMode::LaserDynamic
            && !spindle
                .laser
                .is_some_and(|laser| laser.dynamic_power_supported))
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "requested laser mode is not supported".into(),
        ));
    }
    controller.set_laser_mode(desired);
    Ok(StatusCode::ACCEPTED)
}

/// JSON body for selecting the laser power interpretation.
#[derive(Deserialize)]
pub(crate) struct LaserModeRequest {
    /// Requested `constant` or `dynamic` power behavior.
    mode: String,
}

/// Forwards one command to the selected bridge, returning `None` when a normal adapter is active.
async fn bridge_command(
    state: &SharedController,
    command: &str,
    payload: serde_json::Value,
    timeout: std::time::Duration,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let endpoint = state
        .linuxcnc_endpoint
        .lock()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "LinuxCNC endpoint lock poisoned".into(),
            )
        })?
        .clone();
    let Some(endpoint) = endpoint else {
        return Err((
            StatusCode::CONFLICT,
            "no LinuxCNC bridge is selected".into(),
        ));
    };
    // Centralize request ids/timeouts and preserve remote protocol error codes.
    let response = super::super::state::linuxcnc_bridge::request(
        &endpoint,
        command,
        payload,
        state
            .linuxcnc_request_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        timeout,
    )
    .map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            format!("{}: {}", error.code, error.message),
        )
    })?;
    Ok(response)
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

/// `POST /api/position/select`: activates a coordinate system (G54–G59.3).
pub(crate) async fn select_coordinate_system(
    State(state): State<SharedController>,
    Query(lang): Query<LangQuery>,
    headers: axum::http::HeaderMap,
    Json(request): Json<CoordinateRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let language = request_language(&lang, &headers);
    if let Some(endpoint) = state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
    {
        if ![
            "G54", "G55", "G56", "G57", "G58", "G59", "G59.1", "G59.2", "G59.3",
        ]
        .contains(&request.system.as_str())
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "unsupported coordinate system".into(),
            ));
        }
        super::super::state::linuxcnc_bridge::request(
            &endpoint,
            "set_wcs",
            serde_json::json!({"system":request.system}),
            state
                .linuxcnc_request_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::Duration::from_secs(3),
        )
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.message))?;
        return Ok(StatusCode::ACCEPTED);
    }
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
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        let system = request.system.clone();
        if ![
            "G54", "G55", "G56", "G57", "G58", "G59", "G59.1", "G59.2", "G59.3",
        ]
        .contains(&system.as_str())
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "unsupported coordinate system".into(),
            ));
        }
        bridge_command(
            &state,
            "set_wcs",
            serde_json::json!({"system":system}),
            std::time::Duration::from_secs(3),
        )
        .await?;
        let p = [
            "G54", "G55", "G56", "G57", "G58", "G59", "G59.1", "G59.2", "G59.3",
        ]
        .iter()
        .position(|candidate| *candidate == request.system)
        .unwrap_or(0)
            + 1;
        bridge_command(
            &state,
            "mdi",
            serde_json::json!({"block":format!("G10 L20 P{p} X0 Y0 Z0")}),
            std::time::Duration::from_secs(5),
        )
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
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
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        let axis = axis_letter(request.axis);
        bridge_command(
            &state,
            "mdi",
            serde_json::json!({"block":format!("G10 L20 P1 {axis}0")}),
            std::time::Duration::from_secs(5),
        )
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
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
        bridge_command(
            &state,
            "home",
            serde_json::json!({"axis":axis}),
            std::time::Duration::from_secs(10),
        )
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
    state
        .home_axis(request.axis)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

/// `POST /api/position/home-all`: homes every axis.
pub(crate) async fn home_all(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        bridge_command(
            &state,
            "home",
            serde_json::json!({"axis":"all"}),
            std::time::Duration::from_secs(10),
        )
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
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
    if !request.value.is_finite() {
        return Err((
            StatusCode::BAD_REQUEST,
            "position value must be finite".into(),
        ));
    }
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        let axis = axis_letter(request.axis);
        let system = request.system.clone();
        let systems = [
            "G54", "G55", "G56", "G57", "G58", "G59", "G59.1", "G59.2", "G59.3",
        ];
        let Some(p) = systems.iter().position(|candidate| *candidate == system) else {
            return Err((
                StatusCode::BAD_REQUEST,
                "unsupported coordinate system".into(),
            ));
        };
        bridge_command(
            &state,
            "set_wcs",
            serde_json::json!({"system":system}),
            std::time::Duration::from_secs(3),
        )
        .await?;
        bridge_command(
            &state,
            "mdi",
            serde_json::json!({"block":format!("G10 L20 P{} {axis}{}", p + 1, request.value)}),
            std::time::Duration::from_secs(5),
        )
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
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
    if !request.value.is_finite() {
        return Err((
            StatusCode::BAD_REQUEST,
            "position value must be finite".into(),
        ));
    }
    if state
        .linuxcnc_endpoint
        .lock()
        .ok()
        .and_then(|value| value.clone())
        .is_some()
    {
        let axis = axis_letter(request.axis);
        if ![
            "G54", "G55", "G56", "G57", "G58", "G59", "G59.1", "G59.2", "G59.3",
        ]
        .contains(&request.system.as_str())
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "unsupported coordinate system".into(),
            ));
        }
        bridge_command(
            &state,
            "set_wcs",
            serde_json::json!({"system":request.system}),
            std::time::Duration::from_secs(3),
        )
        .await?;
        bridge_command(
            &state,
            "mdi",
            serde_json::json!({"block":format!("G90 G0 {axis}{}", request.value)}),
            std::time::Duration::from_secs(10),
        )
        .await?;
        return Ok(StatusCode::ACCEPTED);
    }
    state
        .go_to_axis_work_value(request.axis, &request.system, request.value, language)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}
