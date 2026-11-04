use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::cnc_api::Axis;
use crate::web_server::models::{axis_letter, request_language, LangQuery};
use crate::web_server::SharedController;

use super::spindle::bridge_command;
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
        crate::web_server::state::linuxcnc_bridge::request(
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
