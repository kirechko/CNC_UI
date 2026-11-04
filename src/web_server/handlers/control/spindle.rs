use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::web_server::models::{request_language, LangQuery};
use crate::web_server::SharedController;
/// Body of `POST /api/spindle`; legacy `rpm` remains accepted and profile id is descriptive only.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
    #[serde(default)]
    profile_id: Option<String>,
}

/// Supplies clockwise direction when older clients omit a direction for a laser request.
fn default_spindle_direction() -> String {
    "CW".into()
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

    let status = controller.get_status_snapshot();
    if !request.enabled {
        controller.stop_spindle();
        return Ok(StatusCode::ACCEPTED);
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
    let is_laser = spindle.mode != crate::cnc_api::SpindleMode::Standard;
    if let Some(profile_id) = request.profile_id.as_deref() {
        let profiles = state
            .list_machine_profiles()
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
        let profile = profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "selected machine profile does not exist".to_string(),
                )
            })?;
        if matches!(
            profile.machine_type,
            crate::web_server::machine_profiles::MachineType::Laser
                | crate::web_server::machine_profiles::MachineType::Hybrid
        ) && !is_laser
        {
            return Err((
                StatusCode::BAD_REQUEST,
                crate::i18n::translate(language, "api.error.profileLaserMismatch", &[]),
            ));
        }
    }
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
        crate::web_server::state::linuxcnc_bridge::request(
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
pub(super) async fn bridge_command(
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
    let response = crate::web_server::state::linuxcnc_bridge::request(
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
