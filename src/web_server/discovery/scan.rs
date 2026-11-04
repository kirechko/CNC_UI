//! Discovery scan orchestration and its single-flight coordination helpers.

use std::sync::atomic::Ordering;

use super::super::machine::{build_adapter_for_device, emulator_device};
use super::super::state::AppState;
use crate::cnc_controller::CNCController;

/// Tries to take ownership of the one-at-a-time background discovery slot.
pub(super) fn claim_discovery_slot(guard: &std::sync::Arc<std::sync::atomic::AtomicBool>) -> bool {
    !guard.swap(true, Ordering::Acquire)
}

/// Releases the background discovery slot so the next request may rescan.
pub(super) fn release_discovery_slot(guard: &std::sync::Arc<std::sync::atomic::AtomicBool>) {
    guard.store(false, Ordering::Release);
}

/// Adds or refreshes one device in the shared FluidNC registry immediately
/// after it is confirmed by a probe. The whole registry used to be written
/// only when the entire network sweep finished (~25-30 s), so any dialog
/// snapshot taken mid-scan showed just the emulator and the serial GRBL row —
/// the historical "FluidNC stopped being found" symptom. Incremental
/// publishing lets concurrent GET snapshots and the relayed SSE stream below
/// show FluidNC rows as soon as each device answers. The registry is still
/// fully replaced at the end of every scan, which prunes stale entries.
fn publish_registry_device(state: &AppState, device: &cnc_core::discovery::DiscoveredDevice) {
    if let Ok(mut registry) = state.machines.lock() {
        match registry.iter_mut().find(|listed| listed.id == device.id) {
            Some(existing) => *existing = device.clone(),
            None => registry.push(device.clone()),
        }
    }
}

/// Runs the GRBL serial scan and publishes the confirmed ports into
/// `state.serial_machines`. The currently connected machine's port is never
/// opened (probing asserts DTR/RTS and resets Arduino boards, which would
/// interrupt a live control session); its previous record is kept so the
/// dialog still shows the connected row. Serial results deliberately skip the
/// SQLite endpoint cache: that cache seeds *network* priority targets, and a
/// `/dev/...` path is not a resolvable host.
async fn refresh_serial_machines(
    state: &AppState,
    sender: Option<
        &tokio::sync::mpsc::Sender<Result<cnc_core::discovery::DiscoveredDevice, String>>,
    >,
) -> Vec<cnc_core::discovery::DiscoveredDevice> {
    let connected_id = state.connected_machine_id();
    let excluded: Vec<String> = connected_id.clone().into_iter().collect();

    // Keep the connected port's record from the previous scan pass; all other
    // stale entries are replaced by the fresh probe results below.
    let mut devices: Vec<cnc_core::discovery::DiscoveredDevice> = state
        .serial_machines
        .lock()
        .map(|previous| {
            previous
                .iter()
                .filter(|device| connected_id.as_deref() == Some(device.id.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    match cnc_core::discovery::discover_grbl_stream(&excluded).await {
        Ok(mut stream) => {
            while let Some(device) = stream.recv().await {
                if let Some(sender) = sender {
                    if sender.send(Ok(device.clone())).await.is_err() {
                        // SSE consumer disconnected; keep collecting locally.
                    }
                }
                if !devices.iter().any(|listed| listed.id == device.id) {
                    devices.push(device);
                }
            }
        }
        Err(error) => {
            // A platform without serial enumeration (or a busy port manager)
            // must not break the composite machine scan.
            eprintln!("GRBL serial discovery failed: {}", error.message);
        }
    }

    if let Ok(mut serial) = state.serial_machines.lock() {
        *serial = devices.clone();
    }
    devices
}

/// Runs the whole machine search once and pushes every confirmed device to the
/// optional SSE sink while returning the merged list. The emulator is probed
/// first, then the fast GRBL serial scan, and finally the FluidNC network scan
/// prioritizes previously cached endpoints. Future scanners (LinuxCNC) plug in
/// here.
pub(super) async fn discover_all_machines(
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

    // Serial ports answer within ~2 s, so the GRBL row shows up long before
    // the multi-second network sweep finishes.
    for device in refresh_serial_machines(state, sender).await {
        if !found.iter().any(|listed| listed.id == device.id) {
            found.push(device);
        }
    }

    let priority_targets = state.cached_discovery_endpoints().unwrap_or_default();
    match cnc_core::adapters::klipper::discovery::discover(priority_targets.clone()).await {
        Ok(devices) => {
            for device in devices {
                if let Err(error) = state.cache_discovered_endpoint(&device.endpoint) {
                    eprintln!(
                        "Could not persist Moonraker endpoint {}: {error}",
                        device.endpoint
                    );
                }
                publish_registry_device(state, &device);
                if let Some(sender) = sender {
                    let _ = sender.send(Ok(device.clone())).await;
                }
                if !found.iter().any(|listed| listed.id == device.id) {
                    found.push(device);
                }
            }
        }
        Err(error) => eprintln!("Moonraker discovery failed: {}", error.message),
    }
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
                // Publish each confirmed FluidNC into the shared registry
                // right away: GET snapshots taken while this scan runs then
                // already show the machine instead of only the emulator+GRBL.
                publish_registry_device(state, &device);
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

/// Scans on backend startup, connects to the first discovered device, and
/// finishes publishing the complete scan result without blocking HTTP startup.
pub(crate) async fn discover_and_connect_first(state: std::sync::Arc<AppState>) {
    if !claim_discovery_slot(&state.discovery_in_flight) {
        return;
    }

    let (sender, mut receiver) = tokio::sync::mpsc::channel(32);
    let scan_state = state.clone();
    let scanner = tokio::spawn(async move {
        let found = discover_all_machines(&scan_state, Some(&sender)).await;
        if let Ok(mut registry) = scan_state.machines.lock() {
            *registry = found
                .into_iter()
                .filter(|device| device.protocol != "Emulator" && device.protocol != "GRBL")
                .collect();
        }
        release_discovery_slot(&scan_state.discovery_in_flight);
    });

    let mut connected_first_machine = false;
    while let Some(result) = receiver.recv().await {
        let Ok(device) = result else {
            continue;
        };
        if connected_first_machine {
            continue;
        }

        let Ok(adapter) = build_adapter_for_device(&device) else {
            continue;
        };
        let Ok(controller) = CNCController::connect_with_api(adapter) else {
            continue;
        };
        if let Err(error) = state.swap_controller(controller) {
            eprintln!(
                "Could not connect to discovered machine {}: {error}",
                device.endpoint
            );
            continue;
        }
        connected_first_machine = true;

        // Startup discovery may replace the default emulator controller after
        // AppState has already loaded the default program. Restore that program
        // on the newly selected adapter so the viewer and machine stay in sync.
        let previous_program = state
            .program
            .lock()
            .ok()
            .and_then(|program| program.clone());
        if let Some(program) = previous_program {
            let path = super::super::state::position::temp_program_path();
            let result = std::fs::write(&path, &program.content)
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    state
                        .controller
                        .lock()
                        .map_err(|_| "controller lock poisoned".to_string())?
                        .load_program(&path)
                        .map_err(|error| format!("{}: {}", error.code, error.message))
                });
            if let Err(error) = result {
                eprintln!(
                    "Could not reload program `{}` on the startup-selected machine: {error}",
                    program.name
                );
            }
        }

        eprintln!(
            "Connected to first discovered CNC machine: {}",
            device.endpoint
        );
    }

    if let Err(error) = scanner.await {
        eprintln!("Initial machine discovery task failed: {error}");
        release_discovery_slot(&state.discovery_in_flight);
    }
}
