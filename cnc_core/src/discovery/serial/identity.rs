//! Serial identity parsing, stable IDs, and discovered-device construction.

use std::{net::Ipv4Addr, path::Path};

use super::GRBL_DEFAULT_BAUD;
use crate::discovery::{DiscoveredDevice, DiscoveryTransport};

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
pub(super) fn is_supported_grbl_version(version: Option<&str>) -> bool {
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

/// Builds the GRBL-Mega discovery id using the shared GRBL 1.1 wire dialect.
pub fn grbl_mega_serial_device_id(port_path: &str) -> String {
    format!("grbl_mega:serial:{}", serial_port_label(port_path))
}

/// Discovery id of the *serial* FluidNC record a port would produce. Used to
/// exclude the currently connected machine from probing, mirroring
/// [`grbl_serial_device_id`] for the GRBL case.
pub fn fluidnc_serial_device_id(port_path: &str) -> String {
    format!("fluidnc:serial:{}", serial_port_label(port_path))
}

/// Detects a FluidNC boot/identity banner line and extracts the firmware
/// version. Accepted shapes (from real `[Boot:...]`/`[VER:...]` output):
/// `[VER:FluidNC@v1.3.11:Debug]`, `[Boot:FluidNC v1.3.11 build:...`, plus the
/// `[MSG:Machine:...]` companion line. Returns `None` for any GRBL line.
pub fn fluidnc_banner_version(line: &str) -> Option<String> {
    let line = line.trim();
    let inside = line.strip_prefix("[VER:")?;
    if !inside.contains("FluidNC") {
        return None;
    }
    // Version token: first word after "FluidNC", allowing a leading 'v' or
    // '@' decorations the different build scripts emit.
    let after = inside.split("FluidNC").nth(1)?;
    let token = after
        .trim_start_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.')
        .split(|c: char| c == '@' || c == ':' || c == ']' || c == ' ' || c.is_ascii_whitespace())
        .find(|part| !part.is_empty())?;
    let cleaned = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '.');
    // Drop the cosmetic 'v' prefix, matching the network identity parser in
    // `probe.rs::fluidnc_identity`, so both discovery paths report `1.3.11`.
    let version = cleaned
        .strip_prefix('v')
        .or_else(|| cleaned.strip_prefix("version "))
        .unwrap_or(cleaned);
    (!version.is_empty()).then(|| version.to_string())
}

/// Builds the serial FluidNC record: the row the machines dialog selects to
/// control the controller over the COM port (task: FluidNC must be usable via
/// serial). `machine_name` comes from the `[MSG:Machine:…]` identity line so
/// grouping under the multi-interface header matches the network row.
pub(super) fn fluidnc_serial_device(
    port_path: &str,
    version: Option<String>,
    machine_name: Option<String>,
) -> DiscoveredDevice {
    let full_path =
        std::fs::canonicalize(port_path).unwrap_or_else(|_| Path::new(port_path).to_path_buf());
    DiscoveredDevice {
        id: fluidnc_serial_device_id(port_path),
        protocol: "FluidNC".into(),
        machine_name,
        version,
        transport: DiscoveryTransport::Serial,
        endpoint: full_path.to_string_lossy().into_owned(),
    }
}

/// Extracts the first IPv4 address following an `IP` marker in a FluidNC boot
/// stream (e.g. `[MSG:WiFi Connected, IP:192.168.1.42]` or `STA IP:...`).
/// Scanning the accumulated banner (not arbitrary UDP noise) keeps false
/// positives away: only text after an "IP" token qualifies.
pub fn extract_fluidnc_ipv4(text: &str) -> Option<Ipv4Addr> {
    let lower = text.to_ascii_lowercase();
    let mut search_from = 0;
    while let Some(found) = lower[search_from..].find("ip") {
        let start = search_from + found + 2;
        let candidate_start = lower[start..]
            .find(|c: char| c.is_ascii_digit())
            .map(|offset| start + offset)?;
        // Skip "IP" mentions that are not immediately followed by a dotted quad.
        if candidate_start - start > 4 {
            search_from = start;
            continue;
        }
        let rest = &text[candidate_start..];
        let quad: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(address) = quad.parse::<Ipv4Addr>() {
            return Some(address);
        }
        search_from = start;
    }
    None
}

/// Builds a machine-list record for a GRBL-Mega-compatible serial port.
pub fn grbl_mega_serial_device(port_path: &str, version: Option<String>) -> DiscoveredDevice {
    let full_path =
        std::fs::canonicalize(port_path).unwrap_or_else(|_| Path::new(port_path).to_path_buf());
    DiscoveredDevice {
        id: grbl_mega_serial_device_id(port_path),
        protocol: "GRBL-Mega".into(),
        machine_name: None,
        version,
        transport: DiscoveryTransport::Serial,
        endpoint: full_path.to_string_lossy().into_owned(),
    }
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

/// Extracts the `[MSG:Machine:<name>]` value from a captured banner stream.
pub(super) fn machine_name_of(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line
            .trim()
            .strip_prefix("[MSG:Machine:")?
            .strip_suffix(']')?;
        (!value.trim().is_empty()).then(|| value.trim().to_string())
    })
}
