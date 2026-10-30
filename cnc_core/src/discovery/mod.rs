//! Shared CNC device discovery primitives and FluidNC network probes.
//!
//! The result and transport types are deliberately broader than the FluidNC
//! network scanners so serial and local-socket discovery can be added here
//! without adapter-specific result formats.
//!
//! The module is split into children (each well under 500 lines):
//! - [`scan`]    - streaming scan orchestration and the public entry points;
//! - [`targets`] - IPv4 target-set computation, gateway/route heuristics and CIDR math;
//! - [`probe`]   - FluidNC identity probes over WebSocket and Telnet;
//! - [`serial`]  - GRBL discovery over physical COM/serial ports;
//! - [`tests`]   - unit tests (flat, `#[cfg(test)]` only).
//!
//! Public API names are re-exported here so external code keeps using
//! `cnc_core::discovery::*` paths exactly as before the split.

use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

// Re-exported so `crate::discovery::discover_fluidnc_stream` and friends stay
// valid after the implementation moved into the `scan` child module.
pub use scan::{discover_fluidnc, discover_fluidnc_stream, fluidnc_websocket_address};
// GRBL serial scanner entry points and the shared device-id/baud helpers,
// re-exported so callers keep using `cnc_core::discovery::*` paths.
pub use serial::{
    discover_grbl, discover_grbl_stream, fluidnc_serial_device_id, grbl_baud_rate,
    grbl_serial_device, grbl_serial_device_id, parse_banner_version,
};

mod probe;
mod scan;
pub mod serial;
mod targets;
#[cfg(test)]
mod tests;

/// Widest CIDR block the scanner is allowed to enumerate; anything broader
/// (e.g. a whole /16) is rejected to keep a scan bounded. Shared by the CIDR
/// math in `targets` and the gateway expansion in `scan`.
const DISCOVERY_MAX_PREFIX: u8 = 24;

/// Default FluidNC WebSocket/HTTP port used when a discovery config is built.
/// `pub(super)` so the serial scanner can register an ESP32 FluidNC board
/// under its reported Wi-Fi address with the same port convention (task:
/// FluidNC discovery over COM ports).
pub(super) const DEFAULT_FLUIDNC_WEBSOCKET_PORT: u16 = 80;

/// Default FluidNC Telnet port used when a discovery config is built.
const DEFAULT_FLUIDNC_TELNET_PORT: u16 = 23;

/// Connection mechanism for a discovered CNC controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryTransport {
    WebSocket,
    Telnet,
    Serial,
    UnixSocket,
    TcpSocket,
}

impl DiscoveryTransport {
    /// Human-readable transport name for UI listings.
    pub fn label(self) -> &'static str {
        match self {
            Self::WebSocket => "WebSocket",
            Self::Telnet => "Telnet",
            Self::Serial => "COM port",
            Self::UnixSocket => "Unix socket",
            Self::TcpSocket => "TCP socket",
        }
    }
}

/// A controller endpoint found by any discovery backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    /// Stable identifier suitable for comparing results across API layers.
    pub id: String,
    /// Firmware/controller family, for example `FluidNC` or `LinuxCNC`.
    pub protocol: String,
    /// Machine name reported by the controller, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
    /// Firmware version, when reported by the device identity response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub transport: DiscoveryTransport,
    /// Human-readable endpoint; e.g. `192.168.1.20:80`, `/dev/ttyUSB0`, or a socket path.
    pub endpoint: String,
}

impl DiscoveredDevice {
    /// Builds the canonical FluidNC device record from a probe identity.
    /// `pub(super)` so the scan orchestration in the `scan` child and the
    /// unit tests can construct devices without exposing the constructor.
    pub(super) fn fluidnc(
        address: Ipv4Addr,
        port: u16,
        transport: DiscoveryTransport,
        identity: FluidNcIdentity,
    ) -> Self {
        let endpoint = format!("{address}:{port}");
        Self {
            id: format!("fluidnc:{transport:?}:{endpoint}"),
            protocol: "FluidNC".into(),
            machine_name: identity.machine_name,
            version: identity.version,
            transport,
            endpoint,
        }
    }
}

/// Firmware metadata extracted from a FluidNC `$I` identity response.
/// `pub(super)` because it is an argument of the `pub(super)` device
/// constructor shared between the parent module and its children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FluidNcIdentity {
    pub(super) version: Option<String>,
    pub(super) machine_name: Option<String>,
}

/// Network settings for the automatic FluidNC discovery scan.
#[derive(Debug, Clone)]
pub struct FluidNcDiscoveryConfig {
    /// IPv4 hosts or CIDRs to include. CIDRs wider than `/24` are rejected.
    pub explicit_targets: Vec<String>,
    /// Previously discovered endpoints to probe before the general scan.
    pub priority_targets: Vec<String>,
    /// Hosts that must not be probed on any transport, such as a machine that is
    /// already under an active control session (a new probe can disturb it).
    pub excluded_hosts: Vec<Ipv4Addr>,
    pub websocket_port: u16,
    pub telnet_port: u16,
}

impl Default for FluidNcDiscoveryConfig {
    fn default() -> Self {
        Self {
            explicit_targets: Vec::new(),
            priority_targets: Vec::new(),
            excluded_hosts: Vec::new(),
            websocket_port: DEFAULT_FLUIDNC_WEBSOCKET_PORT,
            telnet_port: DEFAULT_FLUIDNC_TELNET_PORT,
        }
    }
}
