//! Opening the serial port and creating the initial session state.

use super::{GrblAdapter, SerialProtocol, Session};
use crate::{
    toolpath::Toolpath, CncResult, ControllerError, CoordinateSystem, SpindleCommand,
    SpindleDirection,
};
use std::collections::HashMap;
use tokio::sync::Mutex;
use tokio_serial::SerialPortBuilderExt;

impl GrblAdapter {
    /// Opens `port_name` at `baud_rate` and prepares an empty session.
    /// The endpoint label stored for discovery is the canonical file name of
    /// the device node (e.g. `ttyUSB0`) when it can be resolved.
    pub fn connect(port_name: &str, baud_rate: u32) -> CncResult<Self> {
        Self::connect_with_protocol(port_name, baud_rate, SerialProtocol::Grbl, None)
    }

    /// Opens `port_name` for a **FluidNC** controller (task: FluidNC over a
    /// COM port). Identical transport to [`Self::connect`] — FluidNC's UART
    /// speaks the GRBL line protocol — but the adapter reports its machine
    /// under the `fluidnc:serial:<port>` discovery id with the banner
    /// machine name, so the machines-dialog row, Rosetta's backend
    /// resolution and the controller highlight all stay consistent.
    pub fn connect_fluidnc(
        port_name: &str,
        baud_rate: u32,
        machine_name: Option<String>,
    ) -> CncResult<Self> {
        Self::connect_with_protocol(port_name, baud_rate, SerialProtocol::FluidNc, machine_name)
    }

    /// Shared port open for both firmware families; see
    /// [`Self::connect`]/[`Self::connect_fluidnc`] for the public entry points.
    fn connect_with_protocol(
        port_name: &str,
        baud_rate: u32,
        protocol: SerialProtocol,
        machine_name: Option<String>,
    ) -> CncResult<Self> {
        let port = tokio_serial::new(port_name, baud_rate)
            .open_native_async()
            .map_err(|error| ControllerError::new("connect", error.to_string()))?;
        let endpoint_path = std::fs::canonicalize(port_name)
            .unwrap_or_else(|_| std::path::PathBuf::from(port_name));
        let serial_endpoint = endpoint_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(port_name)
            .to_string();

        Ok(Self {
            serial_endpoint,
            protocol,
            machine_name,
            session: Mutex::new(Session {
                port,
                protocol,
                program: Vec::new(),
                toolpath: Toolpath::default(),
                last_progress: None,
                program_name: None,
                current_line: 0,
                motion_mode: None,
                active_coordinate_system: CoordinateSystem::G54,
                spindle_command: SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
                min_spindle_rpm: None,
                max_spindle_rpm: None,
                continuous_jog: None,
                rx_buffer: Vec::new(),
                handshake_pending: true,
                // 3 is the stock-GRBL axis count; serial FluidNC updates this
                // from the first wide status report (task: FluidNC COM port).
                reported_axis_count: 3,
                work_offsets: HashMap::new(),
                protocol_poisoned: false,
                program_streaming: false,
                pending_ack: None,
                jog_chunk_active: false,
            }),
        })
    }
}
