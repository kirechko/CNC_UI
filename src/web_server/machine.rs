//! Machine-connection helpers: the local emulator probe and the adapter
//! factory used when the user selects a machine from the discovery list.
//!
//! No adapter configuration file is read anymore — the backend boots with the
//! lazily connecting emulator adapter and every other machine becomes
//! reachable through the machines dialog at runtime.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use crate::cnc_api::CncApi;
use crate::cnc_controller::CNCController;

/// Default loopback endpoint for the bundled software emulator.
const DEFAULT_EMULATOR_ADDRESS: &str = "127.0.0.1:8765";

/// Emulator TCP endpoint, overridable through `CNC_EMULATOR_ADDRESS`.
pub(crate) fn emulator_address() -> String {
    std::env::var("CNC_EMULATOR_ADDRESS").unwrap_or_else(|_| DEFAULT_EMULATOR_ADDRESS.to_string())
}

/// Identifier shared by discovery and `EmAdapter::connected_device_id`.
pub(crate) fn emulator_device_id(address: &str) -> String {
    format!("emulator:tcp_socket:{address}")
}

/// Resolves the configured TCP endpoint and returns an address suitable for connect.
fn resolve_emulator_address(endpoint: &str) -> Option<SocketAddr> {
    endpoint.to_socket_addrs().ok()?.next()
}

/// Returns true when the emulator accepts a TCP connection at the configured endpoint.
pub(crate) fn probe_emulator(endpoint: &str) -> bool {
    let Some(address) = resolve_emulator_address(endpoint) else {
        return false;
    };
    TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok()
}

/// Builds the synthetic machine-list entry for the emulator when its TCP port is reachable.
pub(crate) fn emulator_device() -> Option<cnc_core::discovery::DiscoveredDevice> {
    let endpoint = emulator_address();
    if !probe_emulator(&endpoint) {
        return None;
    }
    Some(cnc_core::discovery::DiscoveredDevice {
        id: emulator_device_id(&endpoint),
        protocol: "Emulator".into(),
        machine_name: Some("CNC Emulator".into()),
        version: None,
        transport: cnc_core::discovery::DiscoveryTransport::TcpSocket,
        endpoint,
    })
}

/// Boots the web backend with the local emulator preselected, so the UI shows
/// live machine data as soon as `cnc-emulator` is running and a clean
/// "offline" state otherwise. The emulator adapter opens its TCP connection lazily,
/// therefore startup never fails because the emulator process is absent; the
/// controller worker keeps polling status and recovers automatically once the
/// emulator appears.
pub(crate) fn default_controller() -> std::io::Result<CNCController> {
    let api: Arc<dyn CncApi> = Arc::new(
        cnc_core::adapters::emulator::EmAdapter::connect(emulator_address())
            .map_err(|error| std::io::Error::other(format!("{}: {}", error.code, error.message)))?,
    );
    CNCController::connect_with_api(api)
        .map_err(|error| std::io::Error::other(format!("{}: {}", error.code, error.message)))
}

/// Constructs the adapter for a user-selected device. Emulator entries carry
/// the TCP endpoint in `endpoint`; network FluidNC entries reuse the
/// discovery adapter priority path so the worker keeps advertising the exact
/// `connected_device_id` that the discovery list advertises; serial FluidNC
/// entries (task: FluidNC over a COM port) open the GRBL-family serial driver
/// in FluidNC mode on the device node; GRBL entries carry the full serial
/// device path (e.g. `/dev/ttyUSB0`) probed by the serial scanner. Other
/// protocols are rejected until their scanners ship.
pub(crate) fn build_adapter_for_device(
    device: &cnc_core::discovery::DiscoveredDevice,
) -> Result<Arc<dyn CncApi>, String> {
    let adapter: Arc<dyn CncApi> = match (device.protocol.as_str(), device.transport) {
        ("Klipper", cnc_core::discovery::DiscoveryTransport::WebSocket) => {
            let mut config = cnc_core::adapters::klipper::KlipperConfig::new(device.endpoint.clone());
            config.api_key = std::env::var("CNC_KLIPPER_API_KEY").ok();
            config.machine_id = Some(device.id.clone());
            Arc::new(cnc_core::adapters::klipper::KlipperAdapter::connect(config)
                .map_err(|error| format!("{}: {}", error.code, error.message))?)
        }
        ("Emulator", cnc_core::discovery::DiscoveryTransport::TcpSocket) => Arc::new(
            cnc_core::adapters::emulator::EmAdapter::connect(device.endpoint.clone())
                .map_err(|error| format!("{}: {}", error.code, error.message))?,
        ),
        // Task: FluidNC over a COM port. The firmware speaks the GRBL line
        // protocol on its console UART, so control runs through the same
        // serial driver in FluidNC family mode: correct `fluidnc:serial:…`
        // identity for the machines dialog, 6-axis status parsing and
        // FluidNC's 250-byte command budget. `endpoint` carries the
        // canonical device node path written by the serial scanner;
        // `machine_name` came from the `[MSG:Machine:…]` identity banner.
        ("FluidNC", cnc_core::discovery::DiscoveryTransport::Serial) => Arc::new(
            cnc_core::adapters::grbl::GrblAdapter::connect_fluidnc(
                &device.endpoint,
                cnc_core::discovery::grbl_baud_rate(),
                device.machine_name.clone(),
            )
            .map_err(|error| format!("{}: {}", error.code, error.message))?,
        ),
        ("FluidNC", cnc_core::discovery::DiscoveryTransport::Telnet) => Arc::new(
            cnc_core::adapters::fluidnc_websocket::FluidNcWebSocketAdapter::connect_telnet(
                device.endpoint.clone(),
            )
            .map_err(|error| format!("{}: {}", error.code, error.message))?,
        ),
        ("FluidNC", _) => Arc::new(
            cnc_core::adapters::fluidnc_websocket::FluidNcWebSocketAdapter::connect_discovering_with_priority(
                cnc_core::adapters::fluidnc_websocket::FluidNcWebSocketConfig::default(),
                vec![device.endpoint.clone()],
            )
            .map_err(|error| format!("{}: {}", error.code, error.message))?,
        ),
        // The scanner stores the canonical full path in `endpoint`, while the
        // adapter derives its `connected_device_id` from the device-node
        // basename; both sides key on that basename, so the selected row
        // highlights exactly like network machines do. The baud matches the
        // scanner rate (`CNC_GRBL_BAUD` override, 115200 by default).
        ("GRBL", cnc_core::discovery::DiscoveryTransport::Serial) => Arc::new(
            cnc_core::adapters::grbl::GrblAdapter::connect(
                &device.endpoint,
                cnc_core::discovery::grbl_baud_rate(),
            )
            .map_err(|error| format!("{}: {}", error.code, error.message))?,
        ),
        (protocol, _) => {
            return Err(format!(
                "connecting to `{protocol}` machines is not supported yet"
            ))
        }
    };
    Ok(adapter)
}
