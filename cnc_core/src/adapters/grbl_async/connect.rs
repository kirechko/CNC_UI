//! Opening the serial port and creating the initial session state.

use super::{GrblAdapter, Session};
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
            session: Mutex::new(Session {
                port,
                program: Vec::new(),
                toolpath: Toolpath::default(),
                program_name: None,
                current_line: 0,
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
                work_offsets: HashMap::new(),
                protocol_poisoned: false,
                program_streaming: false,
                jog_chunk_active: false,
            }),
        })
    }
}
