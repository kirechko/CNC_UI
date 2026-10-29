//! Serial COM-port discovery for GRBL controllers.
//!
//! The scanner enumerates the OS-visible serial ports and opens every
//! plausible candidate at the GRBL line rate. A port is reported as a GRBL
//! machine when the controller proves itself within a short budget:
//! * most Arduino boards reset when the OS asserts DTR/RTS during `open()`
//!   and print a boot banner (`Grbl 1.1h ['$' for help]`), or
//! * a board that does not reset on open (GRBL shields with the reset-cap
//!   mod) still answers the realtime `?` byte with a `<State|...>` status
//!   report, which is the 1.x pipe-separated format this adapter requires.
//!
//! Anything else (plain UARTs, GPS NMEA streams, silent adapters) fails the
//! probe and is not listed. Firmware versions older than GRBL 1.x are
//! deliberately rejected because the async adapter depends on 1.1 protocol
//! features (`$J=` jog, pipe-separated status reports).
//!
//! Device ids are built from the canonical device-node *basename*
//! (`grbl:serial:ttyUSB0`) so they match `GrblAdapter::connected_device_id`
//! byte for byte while the `endpoint` field keeps the full path needed to
//! reopen the port.

use super::{DiscoveredDevice, DiscoveryTransport};
use crate::CncResult;
use std::{
    collections::HashSet,
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinSet};
use tokio_serial::{available_ports, ClearBuffer, SerialPort};

/// Line rate the GRBL scanner opens candidate ports with. Boards shipped
/// after ~2015 default to 115200 baud; older AVR bootloaders are still
/// 9600/57600. Only the configured rate is probed: opening at the wrong rate
/// yields garbled bytes that never match the banner/status grammar, so the
/// port is simply not listed and the operator can override via `CNC_GRBL_BAUD`
/// when a non-default rate is in use.
pub const GRBL_DEFAULT_BAUD: u32 = 115200;

/// Overall probe budget per port: one open, a few `?` pings and the line
/// parsing must finish inside this window so one slow port cannot stall the
/// discovery UI.
const SERIAL_PROBE_DEADLINE: Duration = Duration::from_millis(1800);

/// Blocking read/write timeout of the probe serial port. POSIX returns `Ok(0)`
/// after this idle gap, which the probe loop treats as "no bytes yet".
const SERIAL_PROBE_IO_TIMEOUT: Duration = Duration::from_millis(300);

/// Maximum number of ports probed concurrently. Each probe runs on the
/// blocking thread pool and holds the port for at most the probe deadline.
const SERIAL_MAX_CONCURRENT_PROBES: usize = 8;

/// Guards against a chatty non-GRBL device (e.g. a GPS streaming NMEA)
/// growing the probe buffer without ever matching a GRBL line.
const SERIAL_PROBE_MAX_BUFFER: usize = 8 * 1024;

/// GRBL baud override for the scanner and the machine-selection adapter
/// factory, e.g. `CNC_GRBL_BAUD=9600` for boards still on the legacy rate.
pub fn grbl_baud_rate() -> u32 {
    std::env::var("CNC_GRBL_BAUD")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|rate| *rate > 0)
        .unwrap_or(GRBL_DEFAULT_BAUD)
}

/// Extracts the firmware version token from a GRBL boot banner.
/// `Grbl 1.1h ['$' for help]` -> `Some("1.1h")`; anything that is not a
/// banner line (including words merely starting with `Grbl`) yields `None`.
pub fn parse_banner_version(line: &str) -> Option<String> {
    let rest = line.strip_prefix("Grbl")?;
    // A real banner always separates `Grbl` from the version with whitespace;
    // `Grblby` or similar noise must not masquerade as a controller.
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let token = rest.trim_start().split_whitespace().next()?;
    (!token.is_empty()).then(|| token.to_string())
}

/// True when the version string is a GRBL 1.x firmware. Unknown versions
/// (status-only detection without a banner) are accepted; explicit 0.x
/// releases are rejected because they cannot drive this adapter.
fn is_supported_grbl_version(version: Option<&str>) -> bool {
    match version {
        None => true,
        Some(value) => value.split('.').next().is_some_and(|major| major == "1"),
    }
}

/// Canonical device-node basename used as the stable id key (`ttyUSB0`).
/// Falls back to the raw string when the path cannot be canonicalized (e.g.
/// Windows `COM5` names).
fn serial_port_label(port_path: &str) -> String {
    let resolved =
        std::fs::canonicalize(port_path).unwrap_or_else(|_| Path::new(port_path).to_path_buf());
    resolved
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(port_path)
        .to_string()
}

/// Discovery id of the serial device a given port path would produce.
/// Kept separate from [`grbl_serial_device`] so exclusion lists can be built
/// without canonicalizing twice.
pub fn grbl_serial_device_id(port_path: &str) -> String {
    format!("grbl:serial:{}", serial_port_label(port_path))
}

/// Builds the machine-list record for a confirmed GRBL port. The `endpoint`
/// is the canonical full path (the open target); the `id` follows the
/// basename convention of `GrblAdapter::connected_device_id`, so a connected
/// GRBL machine highlights in the dialog exactly like FluidNC rows do.
pub fn grbl_serial_device(port_path: &str, version: Option<String>) -> DiscoveredDevice {
    let full_path =
        std::fs::canonicalize(port_path).unwrap_or_else(|_| Path::new(port_path).to_path_buf());
    DiscoveredDevice {
        id: grbl_serial_device_id(port_path),
        protocol: "GRBL".into(),
        machine_name: None,
        version,
        transport: DiscoveryTransport::Serial,
        endpoint: full_path.to_string_lossy().into_owned(),
    }
}

/// Port-name prefixes treated as plausible GRBL candidates: FTDI/CH340 USB
/// adapters (`ttyUSB*`), Arduino-UNO-style CDC boards (`ttyACM*`), classic
/// motherboards COMs (`ttyS*`) and the Windows `COMn` spelling. Bluetooth
/// RFCOMM and modem pseudo-devices never carry GRBL and are excluded.
fn is_serial_candidate(port_name: &str) -> bool {
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
fn list_serial_candidates() -> Vec<String> {
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

/// Blocking single-port GRBL probe (runs on `spawn_blocking`).
/// Returns the discovery record when the controller proved itself; `None`
/// for empty, busy, chatty or non-1.x ports.
fn probe_grbl_port_sync(port_path: String, baud: u32) -> Option<DiscoveredDevice> {
    // The probe holds the port only for the deadline window; a port owned by
    // a live adapter session fails the open (or resets the board) and is
    // pre-excluded by the caller via `grbl_serial_device_id`.
    // `timeout` bounds the blocking read; POSIX serial reads then return
    // `Ok(0)` (or EAGAIN-mapped error) after the idle gap instead of hanging.
    let mut port = match tokio_serial::new(&port_path, baud)
        .timeout(SERIAL_PROBE_IO_TIMEOUT)
        .open_native()
    {
        Ok(port) => port,
        // A port held by another process (or a permission error) simply is
        // not a confirmed GRBL machine for this scan pass.
        Err(_) => return None,
    };
    // Drop whatever was buffered before we connected; it is not an answer
    // to our ping.
    let _ = port.clear(ClearBuffer::Input);

    let deadline = Instant::now() + SERIAL_PROBE_DEADLINE;
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 256];
    let mut pings = 0_u32;
    let mut status_seen = false;

    while Instant::now() < deadline {
        // The realtime `?` byte needs no line terminator and is answered
        // with one status report; a board that missed its boot banner on
        // open still reveals itself here.
        if port.write_all(b"?").and_then(|_| port.flush()).is_err() {
            return None;
        }
        pings += 1;

        match port.read(&mut chunk) {
            Ok(0) => {} // idle gap within the probe window: keep waiting
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(_) if Instant::now() >= deadline => break,
            Err(_) => {}
        }
        if buffer.len() > SERIAL_PROBE_MAX_BUFFER {
            // Chatty non-GRBL device; stop collecting and bail out.
            return None;
        }

        // Evaluate every complete line received so far.
        while let Some(line_end) = buffer.iter().position(|byte| *byte == b'\n') {
            let raw: Vec<u8> = buffer.drain(..=line_end).collect();
            let line = String::from_utf8_lossy(&raw[..raw.len() - 1])
                .trim()
                .to_string();

            if let Some(version) = parse_banner_version(&line) {
                if !is_supported_grbl_version(Some(&version)) {
                    tracing::warn!(
                        port = %port_path,
                        version = %version,
                        "skipping serial port: GRBL firmware older than 1.x is not supported"
                    );
                    return None;
                }
                return Some(grbl_serial_device(&port_path, Some(version)));
            }
            // A pipe-separated `<...|...>` report is the 1.x status grammar.
            if line.starts_with('<') && line.ends_with('>') && line.contains('|') {
                status_seen = true;
            }
        }

        // Two answered pings are enough to trust the status-only detection
        // without burning the whole deadline on silent-but-opened ports.
        if status_seen && pings >= 2 {
            break;
        }
    }

    status_seen.then(|| grbl_serial_device(&port_path, None))
}

/// Starts the serial scan and streams confirmed GRBL devices as probes
/// finish. `excluded_ids` carries discovery ids (typically the currently
/// connected machine) whose ports must not be opened — opening a port
/// toggles DTR/RTS and resets Arduino boards, which would interrupt a live
/// control session.
pub async fn discover_grbl_stream(
    excluded_ids: &[String],
) -> CncResult<mpsc::Receiver<DiscoveredDevice>> {
    let candidates = list_serial_candidates();
    let excluded: HashSet<String> = excluded_ids.iter().cloned().collect();
    let candidates: Vec<String> = candidates
        .into_iter()
        .filter(|path| !excluded.contains(&grbl_serial_device_id(path)))
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
        let mut set: JoinSet<Option<DiscoveredDevice>> = JoinSet::new();
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
                Some(Ok(Some(device))) => {
                    let _ = sender.send(device).await;
                }
                Some(Ok(None)) => {}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_version_parses_the_firmware_token_only() {
        assert_eq!(
            parse_banner_version("Grbl 1.1h ['$' for help]").as_deref(),
            Some("1.1h")
        );
        assert_eq!(parse_banner_version("Grbl 0.9j").as_deref(), Some("0.9j"));
        assert_eq!(parse_banner_version("ok"), None);
        assert_eq!(
            parse_banner_version("<Idle|MPos:0.000,0.000,0.000|FS:0,0>"),
            None
        );
        assert_eq!(parse_banner_version("Grblby"), None);
    }

    #[test]
    fn only_grbl_1x_firmware_is_supported() {
        assert!(is_supported_grbl_version(Some("1.1h")));
        assert!(is_supported_grbl_version(Some("1.r2")));
        assert!(!is_supported_grbl_version(Some("0.9j")));
        // Status-only detection has no version and stays acceptable.
        assert!(is_supported_grbl_version(None));
    }

    #[test]
    fn candidate_filter_accepts_common_grbl_adapters_and_skips_others() {
        for name in ["/dev/ttyUSB0", "/dev/ttyACM0", "/dev/ttyS0", "COM5"] {
            assert!(is_serial_candidate(name), "{name}");
        }
        for name in [
            "/dev/ttyprintk",
            "/dev/rfcomm0",
            "/dev/ttyUSB0-modem/Bluetooth",
            "/dev/pts/3",
        ] {
            assert!(!is_serial_candidate(name), "{name}");
        }
    }

    #[test]
    fn device_id_and_endpoint_keep_the_serial_conventions() {
        let device = grbl_serial_device("/dev/ttyUSB0", Some("1.1h".into()));
        // The id key is the canonical basename so it matches
        // GrblAdapter::connected_device_id; the endpoint is the full path.
        assert_eq!(device.id, grbl_serial_device_id("/dev/ttyUSB0"));
        assert!(
            device.id.starts_with("grbl:serial:ttyUSB0"),
            "{}",
            device.id
        );
        assert!(device.endpoint.contains("ttyUSB0"));
        assert_eq!(device.protocol, "GRBL");
        assert_eq!(device.transport, DiscoveryTransport::Serial);
        assert_eq!(device.version.as_deref(), Some("1.1h"));
    }
}
