//! Runtime machine selection and LinuxCNC bridge connection handling.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use std::net::ToSocketAddrs;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::super::machine::build_adapter_for_device;
use super::super::models::{DiscoveredMachine, DiscoveredMachinesResponse, MachineSelectRequest};
use super::super::SharedController;
use super::routes::discovered_machines_response;
use crate::cnc_controller::CNCController;

/// Connect to an explicitly entered LinuxCNC bridge at a host:port endpoint.
async fn select_linuxcnc_bridge(
    state: &std::sync::Arc<super::super::state::AppState>,
    endpoint: String,
) -> Result<Json<DiscoveredMachinesResponse>, (StatusCode, String)> {
    let _: std::net::SocketAddr = endpoint
        .to_socket_addrs()
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?
        .next()
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "LinuxCNC endpoint did not resolve".into(),
            )
        })?;
    let token = std::env::var("LCNC_ADAPTER_TOKEN").ok();
    // The capabilities payload seeds the bridge-specific flat status/configuration mapping.
    let payload = super::super::state::linuxcnc_bridge::request(
        &endpoint,
        "connect",
        serde_json::json!({ "client_name": "CNC_UI", "protocol_version": 1, "token": token }),
        state.linuxcnc_request_id.fetch_add(1, Ordering::Relaxed),
        Duration::from_secs(2),
    )
    .map_err(|error| (StatusCode::BAD_GATEWAY, error.message))?;
    // Persist the endpoint only after the authenticated v1 connect handshake succeeds.
    *state.linuxcnc_endpoint.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "LinuxCNC endpoint lock poisoned".into(),
        )
    })? = Some(endpoint.clone());
    let _ = payload;
    let controller = super::super::machine::default_controller()
        .map_err(|error| (StatusCode::BAD_GATEWAY, error.to_string()))?;
    state
        .swap_controller(controller)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;
    let device = DiscoveredMachine {
        id: format!("linuxcnc:{endpoint}"),
        protocol: "LinuxCNC".into(),
        machine_name: Some("LinuxCNC bridge".into()),
        version: None,
        transport: "TCP".into(),
        address: endpoint.clone(),
        connected: true,
    };
    Ok(Json(DiscoveredMachinesResponse {
        machines: vec![device],
        connected_address: Some(endpoint),
    }))
}

/// `POST /api/machines/select`: switches the backend to the machine whose
/// discovery id the UI selected. Builds a fresh adapter, replaces the
/// controller (the previous worker exits through its now-disconnected command
/// channel) and best-effort reloads the program that was loaded before, so
/// the viewer keeps showing the toolpath.
pub(crate) async fn select_machine(
    State(state): State<SharedController>,
    Json(request): Json<MachineSelectRequest>,
) -> Result<Json<DiscoveredMachinesResponse>, (StatusCode, String)> {
    if let Some(endpoint) = request.id.strip_prefix("linuxcnc:") {
        return select_linuxcnc_bridge(&state, endpoint.to_string()).await;
    }
    let devices = state.machine_snapshot();
    let device = devices
        .iter()
        .find(|listed| listed.id == request.id)
        .cloned()
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!("machine `{}` is not in the discovered list", request.id),
            )
        })?;

    let adapter =
        build_adapter_for_device(&device).map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    let controller = CNCController::connect_with_api(adapter).map_err(|error| {
        (
            StatusCode::BAD_GATEWAY,
            format!("{}: {}", error.code, error.message),
        )
    })?;

    // Remember what was loaded before the switch so the new controller can
    // resume with the same program (adapters keep no program between switches).
    let previous_program = {
        let program = state.program.lock().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "program lock poisoned".to_string(),
            )
        })?;
        program.clone()
    };

    state
        .swap_controller(controller)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;

    // Best-effort: hand the previously loaded program to the newly selected
    // machine through the same temp-file path upload requests use.
    if let Some(program) = previous_program {
        let path = super::super::state::position::temp_program_path();
        if std::fs::write(&path, &program.content).is_ok() {
            // Best-effort: any failure is only logged, never returned to
            // the browser, because the switch itself already succeeded.
            let result = state
                .controller
                .lock()
                .map_err(|_| "controller lock poisoned".to_string())
                .and_then(|controller| {
                    controller
                        .load_program(&path)
                        .map_err(|error| format!("{}: {}", error.code, error.message))
                });
            if let Err(error) = result {
                eprintln!(
                    "Could not reload program `{}` on the selected machine: {error}",
                    program.name
                );
            }
        }
    }

    // The worker thread runs the initial status query right after the swap;
    // give it a short window to report the first successful round trip so
    // this response already marks the newly selected machine as connected.
    // A machine that is actually unreachable only costs the wait budget here.
    // Serial GRBL needs a longer window: the first query also runs the startup
    // handshake that waits for the Arduino boot banner (up to ~3.5 s).
    // Serial GRBL *and* serial FluidNC (task: FluidNC over a COM port) share
    // the same driver family, whose first query also runs the startup
    // handshake waiting for the boot banner (up to ~3.5 s).
    let serial_family = device.protocol == "GRBL"
        || (device.protocol == "FluidNC"
            && device.transport == cnc_core::discovery::DiscoveryTransport::Serial);
    let connect_deadline =
        std::time::Instant::now() + Duration::from_millis(if serial_family { 5000 } else { 1500 });
    let connected_device_id = loop {
        if let Some(id) = state.connected_machine_id() {
            break Some(id);
        }
        if std::time::Instant::now() >= connect_deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    Ok(discovered_machines_response(
        state.machine_snapshot(),
        connected_device_id,
    ))
}
