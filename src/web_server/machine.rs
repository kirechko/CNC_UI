//! Machine-connection helpers: the local emulator probe and the adapter
//! factory used when the user selects a machine from the discovery list.
//!
//! No adapter configuration file is read anymore — the backend boots with the
//! lazily connecting emulator adapter and every other machine becomes
//! reachable through the machines dialog at runtime.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::cnc_api::CncApi;
use crate::cnc_controller::CNCController;

/// Default Unix socket of the bundled software emulator. `cnc-emulator` and
/// the UI default to the same path, so the local machine is discoverable
/// without any configuration file.
const DEFAULT_EMULATOR_SOCKET: &str = "/tmp/linuxcnc-ui-emulator.sock";

/// Emulator socket path, overridable through `CNC_EMULATOR_SOCKET` (tests run
/// a dedicated emulator on a private socket).
pub(crate) fn emulator_socket_path() -> PathBuf {
    std::env::var_os("CNC_EMULATOR_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_EMULATOR_SOCKET))
}

/// Identifier shared by the discovery registry and
/// `EmAdapter::connected_device_id`, so the UI can highlight the emulator row
/// exactly like a FluidNC row.
pub(crate) fn emulator_device_id(socket_path: &Path) -> String {
    format!("emulator:unix_socket:{}", socket_path.display())
}

/// Returns true when a process currently accepts connections on `path`.
/// A stale socket file left behind by a crashed emulator fails fast instead of
/// hanging the HTTP handler.
pub(crate) fn probe_socket(path: &Path) -> bool {
    let Ok(socket) = UnixStream::connect(path) else {
        return false;
    };
    // The probe performs no I/O; the timeouts only bound the blocking socket
    // calls Linux may still perform during connect on a full backlog.
    let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = socket.set_write_timeout(Some(Duration::from_millis(200)));
    drop(socket);
    true
}

/// Builds the synthetic machine-list entry for the local emulator. The endpoint
/// doubles as the connect target for `build_adapter_for_device`, so the entry
/// is listed only while the emulator process actually answers on its socket.
pub(crate) fn emulator_device() -> Option<cnc_core::discovery::DiscoveredDevice> {
    let socket_path = emulator_socket_path();
    if !probe_socket(&socket_path) {
        return None;
    }
    Some(cnc_core::discovery::DiscoveredDevice {
        id: emulator_device_id(&socket_path),
        protocol: "Emulator".into(),
        machine_name: Some("CNC Emulator".into()),
        version: None,
        transport: cnc_core::discovery::DiscoveryTransport::UnixSocket,
        endpoint: socket_path.to_string_lossy().into_owned(),
    })
}

/// Boots the web backend with the local emulator preselected, so the UI shows
/// live machine data as soon as `cnc-emulator` is running and a clean
/// "offline" state otherwise. The emulator adapter opens its socket lazily,
/// therefore startup never fails because the emulator process is absent; the
/// controller worker keeps polling status and recovers automatically once the
/// emulator appears.
pub(crate) fn default_controller() -> std::io::Result<CNCController> {
    let api: Arc<dyn CncApi> = Arc::new(
        cnc_core::adapters::emulator::EmAdapter::connect(emulator_socket_path())
            .map_err(|error| std::io::Error::other(format!("{}: {}", error.code, error.message)))?,
    );
    CNCController::connect_with_api(api)
        .map_err(|error| std::io::Error::other(format!("{}: {}", error.code, error.message)))
}

/// Constructs the adapter for a user-selected device. Emulator entries carry
/// the Unix socket path in `endpoint`; FluidNC entries reuse the discovery
/// adapter priority path so the worker keeps advertising the exact
/// `connected_device_id` that the discovery list advertises; GRBL entries
/// carry the full serial device path (e.g. `/dev/ttyUSB0`) probed by the
/// serial scanner. Other protocols are rejected until their scanners ship.
pub(crate) fn build_adapter_for_device(
    device: &cnc_core::discovery::DiscoveredDevice,
) -> Result<Arc<dyn CncApi>, String> {
    let adapter: Arc<dyn CncApi> = match (device.protocol.as_str(), device.transport) {
        ("Emulator", cnc_core::discovery::DiscoveryTransport::UnixSocket) => Arc::new(
            cnc_core::adapters::emulator::EmAdapter::connect(&device.endpoint)
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
