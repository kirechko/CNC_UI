//! Experimental serial adapters for Marlin and TinyG/TinyG2 firmware.
//!
//! The selected dialect controls request framing and status parsing. Firmware
//! commands remain serialized under one async mutex; unsupported safety or
//! state operations are rejected instead of approximated.

use crate::{CncResult, ControllerError, SpindleCommand, SpindleDirection};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_serial::{SerialPortBuilderExt, SerialStream};

mod api;
mod protocol;

/// Serial wire protocol selected when opening an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialDialect {
    /// Marlin line-oriented G-code console.
    Marlin,
    /// TinyG/TinyG2 JSON-line command protocol.
    TinyG,
}

/// Mutable serial state protected across complete protocol transactions.
pub(super) struct Session {
    /// Open serial port.
    pub(super) port: SerialStream,
    /// Incomplete response bytes retained between line reads.
    pub(super) receive_buffer: Vec<u8>,
    /// Firmware framing selected for this session.
    pub(super) dialect: SerialDialect,
    /// Host-loaded program lines.
    pub(super) program: Vec<String>,
    /// User-visible program name.
    pub(super) program_name: Option<String>,
    /// Number of lines acknowledged by the controller.
    pub(super) current_line: u32,
    /// Last spindle state requested by the application.
    pub(super) spindle: SpindleCommand,
}

/// Experimental serial adapter for a Marlin or TinyG-family controller.
pub struct SerialCompatAdapter {
    /// Protocol dialect for this adapter instance.
    pub(super) dialect: SerialDialect,
    /// Canonical serial device label shown in discovery.
    pub(super) endpoint: String,
    /// Serialized access to the serial session.
    pub(super) session: Mutex<Session>,
}

impl SerialCompatAdapter {
    /// Opens a serial port at the selected baud rate and firmware dialect.
    pub fn connect(port_name: &str, baud_rate: u32, dialect: SerialDialect) -> CncResult<Self> {
        let port = tokio_serial::new(port_name, baud_rate)
            .open_native_async()
            .map_err(|error| ControllerError::new("connect", error.to_string()))?;
        let path = std::fs::canonicalize(port_name)
            .unwrap_or_else(|_| std::path::PathBuf::from(port_name));
        let endpoint = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(port_name)
            .to_owned();
        Ok(Self {
            dialect,
            endpoint,
            session: Mutex::new(Session {
                port,
                receive_buffer: Vec::new(),
                dialect,
                program: Vec::new(),
                program_name: None,
                current_line: 0,
                spindle: SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
            }),
        })
    }

    /// Returns the bounded timeout for an individual command response.
    pub(super) fn response_timeout() -> Duration {
        Duration::from_secs(3)
    }
}
