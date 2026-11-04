//! Serial COM-port discovery for GRBL, GRBL-Mega and FluidNC controllers.
//!
//! This public module path is a facade over cohesive implementation modules:
//! identity/banner helpers, per-port probing, concurrent scanning, and tests.

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
pub(super) const SERIAL_PROBE_DEADLINE: std::time::Duration =
    std::time::Duration::from_millis(1800);

/// Blocking read/write timeout of the probe serial port. POSIX returns `Ok(0)`
/// after this idle gap, which the probe loop treats as "no bytes yet".
pub(super) const SERIAL_PROBE_IO_TIMEOUT: std::time::Duration =
    std::time::Duration::from_millis(300);

/// Maximum number of ports probed concurrently. Each probe runs on the
/// blocking thread pool and holds the port for at most the probe deadline.
pub(super) const SERIAL_MAX_CONCURRENT_PROBES: usize = 8;

/// Guards against a chatty non-GRBL device (e.g. a GPS streaming NMEA)
/// growing the probe buffer without ever matching a GRBL line.
pub(super) const SERIAL_PROBE_MAX_BUFFER: usize = 8 * 1024;

mod identity;
mod probe;
mod scan;
#[cfg(test)]
mod tests;

pub use identity::{
    extract_fluidnc_ipv4, fluidnc_banner_version, fluidnc_serial_device_id, grbl_baud_rate,
    grbl_mega_serial_device, grbl_mega_serial_device_id, grbl_serial_device, grbl_serial_device_id,
    parse_banner_version,
};
pub use scan::{discover_grbl, discover_grbl_stream};
