//! Candidate enumeration and concurrent serial-scan orchestration.

use std::collections::HashSet;
use std::path::Path;

use tokio::{sync::mpsc, task::JoinSet};
use tokio_serial::available_ports;

use super::identity::{fluidnc_serial_device_id, grbl_baud_rate, grbl_serial_device_id};
use super::probe::probe_grbl_port_sync;
use super::SERIAL_MAX_CONCURRENT_PROBES;
use crate::{discovery::DiscoveredDevice, CncResult};

/// Port-name prefixes treated as plausible GRBL candidates: FTDI/CH340 USB
/// adapters (`ttyUSB*`), Arduino-UNO-style CDC boards (`ttyACM*`), classic
/// motherboards COMs (`ttyS*`) and the Windows `COMn` spelling. Bluetooth
/// RFCOMM and modem pseudo-devices never carry GRBL and are excluded.
pub(super) fn is_serial_candidate(port_name: &str) -> bool {
    let name = Path::new(port_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(port_name);
    if name.contains("rfcomm") || name.contains("Bluetooth") {
        return false;
    }
    ["ttyUSB", "ttyACM", "ttyS", "COM"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Enumerates OS serial ports, keeps candidates and dedupes symlink aliases.
/// A platform without serial enumeration is *not* an error for the composite
/// machine scan: it simply yields an empty candidate list.
pub(super) fn list_serial_candidates() -> Vec<String> {
    let Ok(ports) = available_ports() else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    ports
        .into_iter()
        .map(|info| info.port_name)
        .filter(|name| is_serial_candidate(name))
        .filter(|name| {
            let canonical = std::fs::canonicalize(name)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| name.clone());
            seen.insert(canonical)
        })
        .collect()
}

/// Starts the serial scan and streams confirmed GRBL and FluidNC devices as
/// probes finish. `excluded_ids` carries discovery ids (typically the
/// currently connected machine) whose ports must not be opened — opening a
/// port toggles DTR/RTS and resets Arduino/ESP32 boards, which would
/// interrupt a live control session.
pub async fn discover_grbl_stream(
    excluded_ids: &[String],
) -> CncResult<mpsc::Receiver<DiscoveredDevice>> {
    let candidates = list_serial_candidates();
    let excluded: HashSet<String> = excluded_ids.iter().cloned().collect();
    let candidates: Vec<String> = candidates
        .into_iter()
        .filter(|path| {
            !excluded.contains(&grbl_serial_device_id(path))
                && !excluded.contains(&fluidnc_serial_device_id(path))
        })
        .collect();

    let (sender, receiver) = mpsc::channel(candidates.len().max(1));
    let baud = grbl_baud_rate();
    tracing::debug!(
        candidates = candidates.len(),
        baud,
        excluded = excluded.len(),
        "GRBL serial discovery scan started"
    );

    tokio::spawn(async move {
        // One probe may return several rows (serial FluidNC + its optional
        // WebSocket twin), so the join payload is a vector, not an Option.
        let mut set: JoinSet<Vec<DiscoveredDevice>> = JoinSet::new();
        let mut index = 0_usize;

        loop {
            while set.len() < SERIAL_MAX_CONCURRENT_PROBES && index < candidates.len() {
                let path = candidates[index].clone();
                index += 1;
                set.spawn_blocking(move || probe_grbl_port_sync(path, baud));
            }
            if set.is_empty() {
                break;
            }
            match set.join_next().await {
                None => break,
                Some(Ok(devices)) => {
                    for device in devices {
                        let _ = sender.send(device).await;
                    }
                }
                Some(Err(error)) => {
                    // A panicked probe thread must not abort the whole scan.
                    tracing::warn!(%error, "GRBL serial probe task failed");
                }
            }
        }
    });

    Ok(receiver)
}

/// Convenience collector for callers that only need the finished list.
pub async fn discover_grbl(excluded_ids: &[String]) -> CncResult<Vec<DiscoveredDevice>> {
    let mut receiver = discover_grbl_stream(excluded_ids).await?;
    let mut devices = Vec::new();
    while let Some(device) = receiver.recv().await {
        devices.push(device);
    }
    Ok(devices)
}
