//! FluidNC-over-WebSocket adapter (`CncApi` implementation).
//!
//! The original monolithic file was split by responsibility. Rust forbids
//! several `impl CncApi for FluidNcWebSocketAdapter` blocks in one crate
//! (E0119), so the trait surface lives entirely in [`api`]; the other files
//! carry the inherent helper blocks of the same type:
//! * [`connect`]   - constructors, connection lifecycle and discovery tables;
//! * [`transport`] - WebSocket frame send/receive and line framing;
//! * [`query`]     - status, `$$` settings and `$ESP` capability queries;
//! * [`status`]    - status-report parsing and the full status cycle;
//! * [`program`]   - buffered program streaming and program loading;
//! * [`jog`]       - continuous-jog chunking;
//! * [`api`]       - the single `CncApi` trait implementation;
//! * [`tests`]     - unit tests.
//!
//! This module owns the constants, shared types and the adapter struct so
//! every child file sees them through `use super::{...}`.

use crate::discovery::DiscoveredDevice;
use crate::{
    toolpath::ToolpathProgress, Axis, CncEvent, CoordinateSystem, InterpreterState, MachineStatus,
    Position, SpindleCommand, SpindleDirection, SpindleStatus, TaskMode, TaskState,
};
use std::sync::atomic::AtomicBool;
use std::{
    collections::VecDeque,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::{broadcast, watch, Mutex},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const MAX_RESPONSE_LINE: usize = 4096;
const FLUIDNC_PORT: u16 = 80;
const MAX_PROGRAM_BYTES: usize = 5 * 1024 * 1024;
const JOG_CHUNK_MM: f64 = 10.0;
const EVENT_CAPACITY: usize = 256;
/// Consecutive status-query timeouts tolerated before the WebSocket session is dropped.
const STATUS_TIMEOUT_LIMIT: u32 = 3;

/// Normalize lines using the same comment handling and executable-line counting
/// as the browser's G-code trajectory parser.
fn normalized_program_lines(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let mut uncommented = String::with_capacity(line.len());
            let mut remaining = line;
            loop {
                let Some(open) = remaining.find('(') else {
                    uncommented.push_str(remaining);
                    break;
                };
                uncommented.push_str(&remaining[..open]);
                let after_open = open + 1;
                let Some(close) = remaining[after_open..].find(')') else {
                    // Match the browser parser's non-nested comment regex: an
                    // unmatched opening parenthesis is not treated as a comment.
                    uncommented.push_str(&remaining[open..]);
                    break;
                };
                remaining = &remaining[after_open + close + 1..];
            }

            let executable = uncommented.split(';').next().unwrap_or("").trim();
            (!executable.is_empty()).then(|| executable.to_owned())
        })
        .collect()
}

/// Runtime settings for the FluidNC WebSocket adapter.
#[derive(Debug, Clone)]
pub struct FluidNcWebSocketConfig {
    /// Additional IPv4 host addresses or CIDRs to scan (CIDRs must be /24 or narrower).
    pub discovery_port: Vec<String>,
    /// Maximum bytes held in the local unacknowledged-line buffer.
    pub rx_buffer_bytes: usize,
    /// Maximum G-code line length, excluding its newline terminator.
    pub max_line_len: usize,
    /// Timeout waiting for a command acknowledgement.
    pub ack_timeout: Duration,
    /// Timeout waiting for a status response.
    pub status_timeout: Duration,
    /// Timeout establishing a WebSocket connection.
    pub connect_timeout: Duration,
}

impl Default for FluidNcWebSocketConfig {
    fn default() -> Self {
        Self {
            discovery_port: Vec::new(),
            rx_buffer_bytes: 128,
            max_line_len: 80,
            ack_timeout: Duration::from_secs(2),
            status_timeout: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(5),
        }
    }
}

type FluidNcSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Copy)]
struct ContinuousJog {
    x: i8,
    y: i8,
    axis: Option<Axis>,
    feed_rate: f64,
}

struct Session {
    socket: Option<FluidNcSocket>,
    rx_buffer: Vec<u8>,
    work_offset: Option<[f64; 6]>,
    reported_axis_count: usize,
    active_coordinate_system: CoordinateSystem,
    spindle_command: SpindleCommand,
    program: Vec<String>,
    program_name: Option<String>,
    /// Parsed toolpath of the loaded program, used to derive universal progress.
    toolpath: crate::Toolpath,
    /// Next line to send.
    current_line: u32,
    /// Earliest sent line that has not yet received an ACK.
    active_program_line: Option<u32>,
    /// Line numbers and their wire sizes in expected ACK order.
    program_acknowledgements: VecDeque<(u32, usize)>,
    program_acknowledged_bytes: usize,
    program_error: Option<String>,
    program_streaming: bool,
    continuous_jog: Option<ContinuousJog>,
    /// Last progress reported while a program was streaming. Used as the
    /// forward-only floor for the next search; cleared when no program runs.
    last_progress: Option<ToolpathProgress>,
}

fn device_id_host(id: &str) -> Option<std::net::Ipv4Addr> {
    id.split(':').nth(2)?.parse().ok()
}

fn disconnected_status() -> MachineStatus {
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
        coordinate_systems: Vec::new(),
        feed_rate: 0.0,
        spindle: SpindleStatus {
            commanded_rpm: 0.0,
            actual_rpm: 0.0,
            direction: SpindleDirection::Stopped,
            enabled: false,
        },
        mist_coolant_on: false,
        flood_coolant_on: false,
        jog_rate: 0.0,
        motion_mode: None,
        max_spindle_rpm: 0.0,
        loaded_program: None,
        current_line: 0,
        total_lines: 0,
        message: None,
        homed: [false; 6],
        homing: false,
        progress: ToolpathProgress::default(),
    }
}

/// Asynchronous FluidNC WebSocket CNC adapter.
pub struct FluidNcWebSocketAdapter {
    url: Option<String>,
    discovery_enabled: bool,
    config: FluidNcWebSocketConfig,
    discovered_devices: Arc<RwLock<Vec<DiscoveredDevice>>>,
    priority_targets: std::sync::RwLock<Vec<String>>,
    connected_device_id: std::sync::RwLock<Option<String>>,
    /// The first auto-selected machine remains excluded from rediscovery even
    /// after its active connection is lost.
    selected_device_id: std::sync::RwLock<Option<String>>,
    /// Set once a discovered connection succeeds. After that, a lost connection is
    /// not re-established automatically; the user reconnects manually.
    auto_connected: AtomicBool,
    /// Consecutive status-query timeouts on the current session.
    status_timeouts: std::sync::atomic::AtomicU32,
    /// Endpoints of the connected controller's other protocols, read via `$ESP`
    /// queries instead of scanning the same host again.
    verified_capabilities: std::sync::RwLock<Vec<DiscoveredDevice>>,
    session: Mutex<Session>,
    events: broadcast::Sender<CncEvent>,
    status_tx: watch::Sender<MachineStatus>,
}

mod api;
mod connect;
mod jog;
mod program;
mod query;
mod status;
#[cfg(test)]
mod tests;
mod transport;
