//! Machine-discovery endpoints: the instant cached snapshot, the SSE live
//! rescan stream and the runtime machine switch.
//!
//! The search is composite and adapter-independent: it probes the local
//! emulator socket and scans the network for FluidNC controllers regardless
//! of which adapter the controller currently uses. Only one scan runs at a
//! time; the `discovery_in_flight` flag keeps a polling browser from stacking
//! parallel network sweeps.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::machine::{build_adapter_for_device, emulator_device};
use super::models::{DiscoveredMachine, DiscoveredMachinesResponse, MachineSelectRequest};
use super::state::AppState;
use super::SharedController;
use crate::cnc_controller::CNCController;

/// Tries to take ownership of the one-at-a-time background discovery slot.
fn claim_discovery_slot(guard: &std::sync::Arc<std::sync::atomic::AtomicBool>) -> bool {
    !guard.swap(true, Ordering::Acquire)
}

/// Releases the background discovery slot so the next request may rescan.
fn release_discovery_slot(guard: &std::sync::Arc<std::sync::atomic::AtomicBool>) {
    guard.store(false, Ordering::Release);
}

/// Runs the whole machine search once and pushes every confirmed device to the
/// optional SSE sink while returning the merged list. The emulator is probed
/// first, then the FluidNC network scan prioritizes previously cached
/// endpoints. Future scanners (serial COM ports, LinuxCNC) plug in here.
async fn discover_all_machines(
    state: &AppState,
    sender: Option<
        &tokio::sync::mpsc::Sender<Result<cnc_core::discovery::DiscoveredDevice, String>>,
    >,
) -> Vec<cnc_core::discovery::DiscoveredDevice> {
    let mut found: Vec<cnc_core::discovery::DiscoveredDevice> = Vec::new();

    // The emulator appears immediately, even while the network scan is still
    // probing routers and devices.
    if let Some(device) = emulator_device() {
        if let Some(sender) = sender {
            let _ = sender.send(Ok(device.clone())).await;
        }
        found.push(device);
    }

    let priority_targets = state.cached_discovery_endpoints().unwrap_or_default();
    let discovery_config = cnc_core::discovery::FluidNcDiscoveryConfig {
        explicit_targets: Vec::new(),
        priority_targets,
        excluded_hosts: Vec::new(),
        ..cnc_core::discovery::FluidNcDiscoveryConfig::default()
    };
    match cnc_core::discovery::discover_fluidnc_stream(&discovery_config).await {
        Ok(mut stream) => {
            while let Some(device) = stream.recv().await {
                if let Err(error) = state.cache_discovered_endpoint(&device.endpoint) {
                    eprintln!(
                        "Could not persist discovered endpoint {}: {error}",
                        device.endpoint
                    );
                }
                if let Some(sender) = sender {
                    if sender.send(Ok(device.clone())).await.is_err() {
                        // SSE consumer disconnected; keep collecting locally.
                        if !found.iter().any(|listed| listed.id == device.id) {
                            found.push(device);
                        }
                        continue;
                    }
                }
                if !found.iter().any(|listed| listed.id == device.id) {
                    found.push(device);
                }
            }
        }
        Err(error) => {
            // "no IPv4 targets available" and probe failures are not fatal:
            // the emulator entry alone already keeps the machine list usable.
            eprintln!("FluidNC network discovery failed: {}", error.message);
            if let Some(sender) = sender {
                let _ = sender.send(Err(error.message)).await;
            }
        }
    }
    found
}

/// Serializes the device list into the JSON contract of the machines dialog.
pub(crate) fn discovered_machines_response(
    machines: Vec<cnc_core::discovery::DiscoveredDevice>,
    connected_device_id: Option<String>,
) -> Json<DiscoveredMachinesResponse> {
    let connected_address = connected_device_id.as_ref().and_then(|id| {
        machines
            .iter()
            .find(|device| &device.id == id)
            .map(|device| device.endpoint.clone())
    });
    Json(DiscoveredMachinesResponse {
        machines: machines
            .into_iter()
            .map(|device| DiscoveredMachine {
                connected: connected_device_id.as_deref() == Some(device.id.as_str()),
                id: device.id,
                protocol: device.protocol,
                machine_name: device.machine_name,
                version: device.version,
                transport: match device.transport {
                    cnc_core::discovery::DiscoveryTransport::WebSocket => "ws",
                    cnc_core::discovery::DiscoveryTransport::Telnet => "telnet",
                    cnc_core::discovery::DiscoveryTransport::Serial => "serial",
                    cnc_core::discovery::DiscoveryTransport::UnixSocket => "unix",
                    cnc_core::discovery::DiscoveryTransport::TcpSocket => "tcp",
                }
                .into(),
                address: device.endpoint,
            })
            .collect(),
        connected_address,
    })
}

/// `GET /api/discovered-machines`: answer instantly with the live emulator
/// probe plus the cached FluidNC registry, then refresh the network part of
/// the list from a guarded background scan so the dialog never blocks on
/// network probes.
pub(crate) async fn get_discovered_machines(
    State(state): State<SharedController>,
) -> Result<Json<DiscoveredMachinesResponse>, (StatusCode, String)> {
    let payload = discovered_machines_response(
        state.machine_snapshot(),
        state.connected_machine_id(),
    );
    if claim_discovery_slot(&state.discovery_in_flight) {
        let background_state = state.clone();
        tokio::spawn(async move {
            let devices = discover_all_machines(&background_state, None).await;
            // Only network devices persist in the registry; the emulator entry
            // is recomputed live at every snapshot.
            if let Ok(mut registry) = background_state.machines.lock() {
                *registry = devices
                    .into_iter()
                    .filter(|device| device.protocol != "Emulator")
                    .collect();
            }
            release_discovery_slot(&background_state.discovery_in_flight);
        });
    }
    Ok(payload)
}

/// `POST /api/discovered-machines`: live rescan as a server-sent-event stream
/// so the dialog renders machines as they are confirmed.
pub(crate) async fn rediscover_machines(
    State(state): State<SharedController>,
) -> Result<
    axum::response::sse::Sse<
        std::pin::Pin<
            Box<
                dyn futures_util::Stream<
                        Item = Result<axum::response::sse::Event, std::convert::Infallible>,
                    > + Send,
            >,
        >,
    >,
    (StatusCode, String),
> {
    use axum::response::sse::{Event, Sse};
    use futures_util::stream;
    use std::convert::Infallible;

    let snapshot = state.machine_snapshot();
    let connected_device_id = state.connected_machine_id();

    // Only one live scan runs at a time; a browser that retries must not stack
    // parallel network sweeps. The guard is released by the stream below.
    let claim_held = claim_discovery_slot(&state.discovery_in_flight);
    if !claim_held || snapshot.is_empty() {
        if claim_held {
            release_discovery_slot(&state.discovery_in_flight);
        }
        // No scan possible (one already runs) or nothing is known yet: emit
        // the current state as a complete event so the dialog still renders.
        let payload = discovered_machines_response(snapshot, connected_device_id);
        let event = Event::default()
            .event("complete")
            .json_data(payload.0)
            .unwrap();
        let events: std::pin::Pin<
            Box<dyn futures_util::Stream<Item = Result<Event, Infallible>> + Send>,
        > = Box::pin(stream::iter(vec![Ok(event)]));
        return Ok(Sse::new(events));
    }

    let guard = state.discovery_in_flight.clone();
    let (sender, mut receiver) =
        tokio::sync::mpsc::channel::<Result<cnc_core::discovery::DiscoveredDevice, String>>(32);
    let scan_state = state.clone();
    // The scan task owns the release of the discovery guard and the registry
    // refresh: if the SSE consumer disconnects midway, the stream future is
    // dropped before it could release the slot, and a task-local release keeps
    // later scans from being blocked forever.
    let scanner = tokio::spawn(async move {
        let found = discover_all_machines(&scan_state, Some(&sender)).await;
        if let Ok(mut registry) = scan_state.machines.lock() {
            *registry = found
                .iter()
                .filter(|device| device.protocol != "Emulator")
                .cloned()
                .collect();
        }
        release_discovery_slot(&guard);
        found
    });
    let state_for_stream = state.clone();
    let initial_response = discovered_machines_response(snapshot.clone(), connected_device_id);
    let snapshot_event = Event::default()
        .event("snapshot")
        .json_data(initial_response.0)
        .unwrap();
    let events = async_stream::stream! {
        yield Ok::<_, Infallible>(snapshot_event);
        let mut devices = snapshot;
        let mut discovery_error: Option<String> = None;
        while let Some(result) = receiver.recv().await {
            match result {
                Ok(device) => {
                    if !devices.iter().any(|listed| listed.id == device.id) {
                        devices.push(device);
                    }
                }
                Err(error) => discovery_error = Some(error),
            }
            let response = discovered_machines_response(
                devices.clone(),
                state_for_stream.connected_machine_id(),
            );
            let event = Event::default().event("machines").json_data(response.0).unwrap();
            yield Ok::<_, Infallible>(event);
        }
        // The scanner already published the registry and released the guard.
        let _ = scanner.await;
        // Merge the freshly scanned list with the live snapshot for the final
        // event. A discovery error only surfaces when no machine at all was
        // found; otherwise the (possibly emulator-only) list is still useful.
        let mut final_devices = state_for_stream.machine_snapshot();
        for device in &devices {
            if !final_devices.iter().any(|listed| listed.id == device.id) {
                final_devices.push(device.clone());
            }
        }
        if let Some(error) = discovery_error {
            if final_devices.is_empty() {
                let event = Event::default().event("error").data(error);
                yield Ok::<_, Infallible>(event);
            }
        }
        let response = discovered_machines_response(
            final_devices,
            state_for_stream.connected_machine_id(),
        );
        let event = Event::default().event("complete").json_data(response.0).unwrap();
        yield Ok::<_, Infallible>(event);
    };
    let events: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<Event, Infallible>> + Send>,
    > = Box::pin(events);
    Ok(Sse::new(events).keep_alive(axum::response::sse::KeepAlive::default()))
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

    let adapter = build_adapter_for_device(&device)
        .map_err(|error| (StatusCode::BAD_GATEWAY, error))?;
    let controller = CNCController::connect_with_api(adapter)
        .map_err(|error| (StatusCode::BAD_GATEWAY, format!("{}: {}", error.code, error.message)))?;

    // Remember what was loaded before the switch so the new controller can
    // resume with the same program (adapters keep no program between switches).
    let previous_program = {
        let program = state
            .program
            .lock()
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "program lock poisoned".to_string()))?;
        program.clone()
    };

    state
        .swap_controller(controller)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))?;

    // Best-effort: hand the previously loaded program to the newly selected
    // machine through the same temp-file path upload requests use.
    if let Some(program) = previous_program {
        let path = super::state::position::temp_program_path();
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
    let connect_deadline = std::time::Instant::now() + Duration::from_millis(1500);
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
