//! Moonraker-backed Klipper adapter. The implementation is split by protocol
//! responsibility so transport, state mapping, commands, capabilities and the
//! `CncApi` facade remain independently reviewable.

use crate::discovery::DiscoveredDevice;
use crate::{
    Axis, CncEvent, CoordinateSystem, InterpreterState, MachineStatus, Position, SpindleCommand,
    SpindleDirection, SpindleMode, SpindleStatus, TaskMode, TaskState, ToolpathProgress,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch, Mutex};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// Moonraker connection settings. The API key is optional for trusted networks.
#[derive(Debug, Clone)]
pub struct KlipperConfig {
    /// Moonraker base URL, for example `http://printer.local:7125`.
    pub endpoint: String,
    /// Optional Moonraker API key used for WebSocket and REST authentication.
    pub api_key: Option<String>,
    /// Maximum time allowed for connecting and waiting for an RPC response.
    pub request_timeout: Duration,
    /// Stable ID from a discovery entry; keeps selection identity aligned across rescans.
    pub machine_id: Option<String>,
}

impl KlipperConfig {
    /// Creates a configuration with the standard Moonraker port and timeout.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: None,
            request_timeout: Duration::from_secs(10),
            machine_id: None,
        }
    }
}

impl Default for KlipperConfig {
    /// Returns a safe default configuration for an explicitly provided endpoint.
    fn default() -> Self {
        Self::new("http://127.0.0.1:7125")
    }
}

type KlipperSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Mutable protocol state protected by the adapter's asynchronous mutex.
pub(super) struct Session {
    /// Active Moonraker JSON-RPC WebSocket, opened lazily on the first operation.
    pub(super) socket: Option<KlipperSocket>,
    /// Monotonically increasing JSON-RPC request identifier.
    pub(super) next_id: u64,
    /// Most recent printer object values received from query or subscription.
    pub(super) objects: serde_json::Map<String, serde_json::Value>,
    /// Object names discovered from `printer.objects.list`.
    pub(super) object_names: Vec<String>,
    /// Name and version from `server.info`.
    pub(super) machine_name: Option<String>,
    /// Cached configuration settings from `configfile.settings`.
    pub(super) config_settings: serde_json::Value,
    /// Currently selected laser output object, if one is configured/detected.
    pub(super) laser_pin: Option<String>,
    /// Last commanded spindle/laser state, used when the firmware has no status object.
    pub(super) spindle_command: SpindleCommand,
    /// Cached program path used by the UI while print_stats is between updates.
    pub(super) program_name: Option<String>,
    /// Parsed toolpath used to map the live tool position to physical path progress.
    pub(super) toolpath: crate::Toolpath,
    /// Executable line index used to resume Rosetta's streamed G-code blocks.
    pub(super) script_line: u32,
    /// Total executable G-code lines for the uploaded program's fallback progress estimate.
    pub(super) total_program_lines: u32,
    /// Next continuous-jog segment is queued from status refreshes until cancelled.
    pub(super) continuous_jog: Option<(Axis, i8, f64)>,
    /// Whether the current Klippy instance has an active printer-object subscription.
    pub(super) subscribed: bool,
    /// Whether this session has entered Klippy shutdown after E-stop.
    pub(super) estopped: bool,
}

/// Asynchronous Moonraker client implementing the shared CNC control API.
#[derive(Clone)]
pub struct KlipperAdapter {
    /// Immutable connection and timeout configuration.
    pub(super) config: KlipperConfig,
    /// Stable identity of the selected Moonraker endpoint.
    pub(super) device: DiscoveredDevice,
    /// Serializes WebSocket request/response exchanges and owns the socket.
    pub(super) session: Arc<Mutex<Session>>,
    /// Last status snapshot exposed without network I/O.
    pub(super) status_tx: watch::Sender<MachineStatus>,
    /// Adapter events consumed by the controller/UI layers.
    pub(super) events: broadcast::Sender<CncEvent>,
    /// Last error delivered by `notify_gcode_response` or Klipper lifecycle events.
    pub(super) last_error: Arc<Mutex<Option<String>>>,
}

/// Builds the disconnected status snapshot used before Moonraker is reached.
pub(super) fn disconnected_status() -> MachineStatus {
    MachineStatus {
        connected: false,
        task_mode: TaskMode::Manual,
        task_state: TaskState::Off,
        interpreter_state: InterpreterState::Idle,
        position: Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            a: 0.0,
            b: 0.0,
            c: 0.0,
        },
        work_offset: Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            a: 0.0,
            b: 0.0,
            c: 0.0,
        },
        active_coordinate_system: CoordinateSystem::G54,
        coordinate_systems: vec![crate::CoordinateSystemOffset {
            system: CoordinateSystem::G54,
            offset: Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                a: 0.0,
                b: 0.0,
                c: 0.0,
            },
        }],
        feed_rate: 0.0,
        spindle: SpindleStatus {
            commanded_rpm: 0.0,
            actual_rpm: 0.0,
            direction: SpindleDirection::Stopped,
            enabled: false,
            mode: SpindleMode::Standard,
            laser_power: None,
        },
        mist_coolant_on: false,
        flood_coolant_on: false,
        jog_rate: 0.0,
        motion_mode: None,
        max_spindle_rpm: 0.0,
        probe_triggered: false,
        probe_position: None,
        selected_tool: None,
        current_tool: None,
        loaded_program: None,
        current_line: 0,
        total_lines: 0,
        message: None,
        homed: [false; 6],
        homing: false,
        progress: ToolpathProgress::default(),
    }
}

mod api;
mod capabilities;
mod commands;
pub mod discovery;
pub mod files;
mod jog;
mod laser;
mod rosetta;
mod rpc;
mod status;

#[cfg(test)]
mod tests;
