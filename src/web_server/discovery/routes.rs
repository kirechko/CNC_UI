//! HTTP discovery snapshot and SSE rescan handlers.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::super::models::{DiscoveredMachine, DiscoveredMachinesResponse};
use super::super::SharedController;
use super::scan::{claim_discovery_slot, discover_all_machines, release_discovery_slot};

/// Serializes the device list into the JSON contract of the machines dialog.
pub(super) fn discovered_machines_response(
    machines: Vec<cnc_core::discovery::DiscoveredDevice>,
    connected_device_id: Option<String>,
) -> Json<DiscoveredMachinesResponse> {
    // The live bridge endpoint is held by AppState, not the shared discovery registry;
    // this snapshot conversion therefore only serializes scanner results.
    let connected_address = connected_device_id.as_ref().and_then(|id| {
        machines
            .iter()
            .find(|device| &device.id == id)
            .map(|device| device.endpoint.clone())
    });
    let listed = machines
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
        .collect::<Vec<_>>();
    Json(DiscoveredMachinesResponse {
        machines: listed,
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
    let payload =
        discovered_machines_response(state.machine_snapshot(), state.connected_machine_id());
    if claim_discovery_slot(&state.discovery_in_flight) {
        let background_state = state.clone();
        tokio::spawn(async move {
            let devices = discover_all_machines(&background_state, None).await;
            // Only network devices persist in the registry; the emulator entry
            // is recomputed live at every snapshot and serial devices live in
            // their own `serial_machines` store (already refreshed in the scan).
            if let Ok(mut registry) = background_state.machines.lock() {
                *registry = devices
                    .into_iter()
                    .filter(|device| device.protocol != "Emulator" && device.protocol != "GRBL")
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
    if snapshot.is_empty() {
        if claim_held {
            release_discovery_slot(&state.discovery_in_flight);
        }
        // Nothing is known yet: emit the current state as a complete event so
        // the dialog still renders.
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

    if !claim_held {
        // Another scan already owns the slot. Answering with a single stale
        // snapshot here was the "FluidNC is no longer found" symptom: mid-scan
        // that snapshot contains only the emulator and the fast serial GRBL
        // row, while the FluidNC probes may still be many seconds away.
        // Instead, relay the shared registry — the running scanner publishes
        // every confirmed device incrementally — until it releases the slot,
        // then close with a final complete event. The deadline bounds the wait
        // in case the scanner task ever panics and leaves the flag set.
        let watch_state = state.clone();
        let guard = state.discovery_in_flight.clone();
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        let events = async_stream::stream! {
            let mut last_ids: Option<String> = None;
            loop {
                let devices = watch_state.machine_snapshot();
                let ids = devices
                    .iter()
                    .map(|device| device.id.as_str())
                    .collect::<Vec<_>>()
                    .join("|");
                if last_ids.as_deref() != Some(ids.as_str()) {
                    last_ids = Some(ids);
                    let response = discovered_machines_response(
                        devices,
                        watch_state.connected_machine_id(),
                    );
                    let event =
                        Event::default().event("machines").json_data(response.0).unwrap();
                    yield Ok::<_, Infallible>(event);
                }
                if !guard.load(Ordering::Acquire) || std::time::Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(400)).await;
            }
            let response = discovered_machines_response(
                watch_state.machine_snapshot(),
                watch_state.connected_machine_id(),
            );
            let event = Event::default().event("complete").json_data(response.0).unwrap();
            yield Ok::<_, Infallible>(event);
        };
        let events: std::pin::Pin<
            Box<dyn futures_util::Stream<Item = Result<Event, Infallible>> + Send>,
        > = Box::pin(events);
        return Ok(Sse::new(events).keep_alive(axum::response::sse::KeepAlive::default()));
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
                .filter(|device| device.protocol != "Emulator" && device.protocol != "GRBL")
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
