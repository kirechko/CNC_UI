use crate::discovery::{
    discover_fluidnc_stream, fluidnc_websocket_address, DiscoveredDevice, DiscoveryTransport,
    FluidNcDiscoveryConfig,
};
use crate::{
    machine_state, Axis, AxisConfiguration, AxisUnits, CncApi, CncEvent, CncResult,
    ControllerError, CoordinateSystem, InterpreterState, JogRequest, MachineAxis,
    MachineConfiguration, MachineState, MachineStatus, OverrideKind, Position, ProgramSource,
    SpindleCommand, SpindleConfiguration, SpindleDirection, SpindleStatus, TaskMode, TaskState,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{Arc, OnceLock, RwLock},
    time::{Duration, Instant},
};
use tokio::{
    net::TcpStream,
    sync::{broadcast, watch, Mutex},
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

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
    /// Следующая строка для отправки.
    current_line: u32,
    /// Самая ранняя отправленная строка, для которой ещё не получен ACK.
    active_program_line: Option<u32>,
    /// Номера строк и их wire-size в порядке ожидаемых ACK.
    program_acknowledgements: VecDeque<(u32, usize)>,
    program_acknowledged_bytes: usize,
    program_error: Option<String>,
    program_streaming: bool,
    continuous_jog: Option<ContinuousJog>,
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

impl FluidNcWebSocketAdapter {
    /// Create a FluidNC adapter with the proposed default runtime settings.
    pub fn connect(url: impl Into<String>) -> CncResult<Self> {
        Self::connect_with_config(url, FluidNcWebSocketConfig::default())
    }

    /// Create an adapter that scans for FluidNC when connecting and reconnecting.
    pub fn connect_discovering(config: FluidNcWebSocketConfig) -> CncResult<Self> {
        Self::connect_discovering_with_priority(config, Vec::new())
    }

    /// Create a discovery adapter, prioritizing previously found WebSocket endpoints.
    pub fn connect_discovering_with_priority(
        config: FluidNcWebSocketConfig,
        priority_targets: Vec<String>,
    ) -> CncResult<Self> {
        let adapter = Self::construct(None, config, true)?;
        adapter
            .priority_targets
            .write()
            .map_err(|_| ControllerError::new("discovery", "priority target lock poisoned"))?
            .extend(priority_targets);
        Ok(adapter)
    }

    /// Create a FluidNC adapter with explicit runtime settings.
    pub fn connect_with_config(
        url: impl Into<String>,
        config: FluidNcWebSocketConfig,
    ) -> CncResult<Self> {
        Self::construct(Some(url.into()), config, false)
    }

    fn construct(
        url: Option<String>,
        config: FluidNcWebSocketConfig,
        discovery_enabled: bool,
    ) -> CncResult<Self> {
        if config.rx_buffer_bytes == 0 || config.max_line_len == 0 {
            return Err(ControllerError::new(
                "configuration",
                "FluidNC receive buffer and maximum line length must be nonzero",
            ));
        }
        if config.ack_timeout.is_zero()
            || config.status_timeout.is_zero()
            || config.connect_timeout.is_zero()
        {
            return Err(ControllerError::new(
                "configuration",
                "FluidNC timeouts must be greater than zero",
            ));
        }
        if !discovery_enabled
            && !url
                .as_deref()
                .is_some_and(|url| url.starts_with("ws://") || url.starts_with("wss://"))
        {
            return Err(ControllerError::new(
                "connect",
                "FluidNC WebSocket URL must start with ws:// or wss://",
            ));
        }

        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let (status_tx, _) = watch::channel(disconnected_status());
        Ok(Self {
            url,
            discovery_enabled,
            config,
            discovered_devices: Arc::new(RwLock::new(Vec::new())),
            priority_targets: std::sync::RwLock::new(Vec::new()),
            connected_device_id: std::sync::RwLock::new(None),
            selected_device_id: std::sync::RwLock::new(None),
            auto_connected: AtomicBool::new(false),
            status_timeouts: std::sync::atomic::AtomicU32::new(0),
            verified_capabilities: std::sync::RwLock::new(Vec::new()),
            session: Mutex::new(Session {
                socket: None,
                rx_buffer: Vec::new(),
                work_offset: None,
                reported_axis_count: 3,
                active_coordinate_system: CoordinateSystem::G54,
                spindle_command: SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
                program: Vec::new(),
                program_name: None,
                current_line: 0,
                active_program_line: None,
                program_acknowledgements: VecDeque::new(),
                program_acknowledged_bytes: 0,
                program_error: None,
                program_streaming: false,
                continuous_jog: None,
            }),
            events,
            status_tx,
        })
    }

    fn publish_status(&self, status: MachineStatus) {
        let previous = self.status_tx.borrow().clone();
        let previous_state = machine_state(&previous);
        let next_state = machine_state(&status);
        if previous_state != next_state {
            let _ = self.events.send(CncEvent::StateChanged {
                from: previous_state,
                to: next_state,
            });
        }
        if previous.current_line != status.current_line {
            let _ = self.events.send(CncEvent::ProgramLineChanged {
                line: status.current_line,
            });
        }
        if next_state == MachineState::Alarm && previous_state != MachineState::Alarm {
            let _ = self.events.send(CncEvent::Alarm {
                code: "fluidnc_alarm".into(),
                message: status
                    .message
                    .clone()
                    .unwrap_or_else(|| "FluidNC alarm".into()),
            });
        }
        self.status_tx.send_replace(status);
        let _ = self.events.send(CncEvent::StatusUpdate);
    }

    pub fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        let selected_host: Option<std::net::Ipv4Addr> = self
            .selected_device_id
            .read()
            .ok()
            .and_then(|id| id.as_deref().and_then(device_id_host));
        let mut devices: Vec<DiscoveredDevice> = self
            .discovered_devices
            .read()
            .map(|devices| {
                devices
                    .iter()
                    .filter(|device| {
                        let device_host = device
                            .endpoint
                            .rsplit_once(':')
                            .map_or(device.endpoint.as_str(), |(host, _)| host)
                            .parse::<std::net::Ipv4Addr>()
                            .ok();
                        selected_host != device_host
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        // Verified entries of the connected controller are shown even though its
        // scan results are hidden.
        if let Ok(verified) = self.verified_capabilities.read() {
            for device in verified.iter() {
                if !devices.iter().any(|existing| existing.id == device.id) {
                    devices.push(device.clone());
                }
            }
        }
        devices
    }

    pub fn connected_device_id(&self) -> Option<String> {
        self.connected_device_id
            .read()
            .ok()
            .and_then(|id| id.clone())
    }

    async fn ensure_connected(&self, session: &mut Session) -> CncResult<()> {
        if session.socket.is_some() {
            return Ok(());
        }

        if self.discovery_enabled && self.auto_connected.load(Ordering::SeqCst) {
            return Err(ControllerError::new(
                "disconnected",
                "FluidNC connection was lost; reconnect manually",
            ));
        }

        if !self.discovery_enabled {
            let url = self.url.clone().ok_or_else(|| {
                ControllerError::new("connect", "FluidNC WebSocket URL is not configured")
            })?;
            let (socket, _) = timeout(self.config.connect_timeout, connect_async(&url))
                .await
                .map_err(|_| ControllerError::new("timeout", "timed out connecting to FluidNC"))?
                .map_err(|error| ControllerError::new("connect", error.to_string()))?;
            session.socket = Some(socket);
            tracing::info!(%url, "connected to configured FluidNC WebSocket");
            let _ = self.events.send(CncEvent::Connected);
            session.rx_buffer.clear();
            return Ok(());
        }

        let priority_targets = self
            .priority_targets
            .read()
            .map(|targets| targets.clone())
            .unwrap_or_default();
        for endpoint in &priority_targets {
            let Some((host, port)) = endpoint.rsplit_once(':') else {
                continue;
            };
            // Cached Telnet entries (port 23) identify the same FluidNC host, whose
            // WebSocket is served on FLUIDNC_PORT, so they are valid fast-path hosts.
            if !matches!(port.parse::<u16>().ok(), Some(p) if p == FLUIDNC_PORT || p == 23) {
                continue;
            }
            let Ok(address) = host.parse::<std::net::Ipv4Addr>() else {
                continue;
            };
            let url = format!("ws://{address}:{FLUIDNC_PORT}/");
            match timeout(self.config.connect_timeout, connect_async(&url)).await {
                Ok(Ok((socket, _))) => {
                    session.socket = Some(socket);
                    self.auto_connected.store(true, Ordering::SeqCst);
                    let id = format!("fluidnc:WebSocket:{address}:{FLUIDNC_PORT}");
                    if let Ok(mut connected) = self.connected_device_id.write() {
                        *connected = Some(id.clone());
                    }
                    if let Ok(mut selected) = self.selected_device_id.write() {
                        *selected = Some(id.clone());
                    }
                    // No discovery runs while a session is active: the connected
                    // controller is read via $ESP queries, not probed again.
                    self.query_protocol_capabilities(session, address).await;
                    tracing::info!(%url, "connected directly to cached FluidNC endpoint");
                    let _ = self.events.send(CncEvent::Connected);
                    session.rx_buffer.clear();
                    return Ok(());
                }
                Ok(Err(error)) => {
                    tracing::debug!(%url, %error, "cached FluidNC endpoint is stale; scanning again")
                }
                Err(_) => {
                    tracing::debug!(%url, "cached FluidNC endpoint timed out; scanning again")
                }
            }
        }

        let mut discoveries = discover_fluidnc_stream(&FluidNcDiscoveryConfig {
            explicit_targets: self.config.discovery_port.clone(),
            priority_targets,
            excluded_hosts: Vec::new(),
            websocket_port: FLUIDNC_PORT,
            telnet_port: 23,
        })
        .await?;
        let devices_shared = self.discovered_devices.clone();
        if let Ok(mut connected) = self.connected_device_id.write() {
            *connected = None;
        }

        // Take the first successful WebSocket identity result immediately. Once
        // connected, the scan receiver is dropped so no further probes are sent
        // while the session is active.
        while let Some(device) = discoveries.recv().await {
            if let Ok(mut devices) = devices_shared.write() {
                if !devices.iter().any(|existing| existing.id == device.id) {
                    devices.push(device.clone());
                }
            }
            if device.transport != DiscoveryTransport::WebSocket {
                continue;
            }
            let Some(address) = fluidnc_websocket_address(&device) else {
                continue;
            };
            let url = format!("ws://{address}:{FLUIDNC_PORT}/");
            tracing::info!(%url, "connecting to first confirmed FluidNC WebSocket");
            match timeout(self.config.connect_timeout, connect_async(&url)).await {
                Ok(Ok((socket, _))) => {
                    session.socket = Some(socket);
                    self.auto_connected.store(true, Ordering::SeqCst);
                    if let Ok(mut connected) = self.connected_device_id.write() {
                        *connected = Some(device.id.clone());
                    }
                    if let Ok(mut selected) = self.selected_device_id.write() {
                        *selected = Some(device.id.clone());
                    }
                    self.query_protocol_capabilities(session, address).await;
                    // Stop the scan: an active session must not receive parallel probes.
                    drop(discoveries);
                    tracing::info!(%url, "connected to first discovered FluidNC");
                    let _ = self.events.send(CncEvent::Connected);
                    session.rx_buffer.clear();
                    return Ok(());
                }
                Ok(Err(error)) => {
                    tracing::warn!(%url, %error, "failed to connect to discovered FluidNC; trying next match");
                }
                Err(_) => {
                    tracing::warn!(%url, "timed out connecting to discovered FluidNC; trying next match");
                }
            }
        }
        Err(ControllerError::new(
            "discovery",
            "no usable FluidNC WebSocket endpoint found on TCP port 80",
        ))
    }

    #[track_caller]
    fn invalidate_connection(&self, session: &mut Session) {
        let was_connected = session.socket.is_some();
        if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
            let caller = std::panic::Location::caller();
            eprintln!(
                "[FluidNC WS] invalidate_connection called from {}:{} (was_connected={})",
                caller.file(),
                caller.line(),
                was_connected
            );
        }
        session.socket = None;
        if let Ok(mut connected) = self.connected_device_id.write() {
            *connected = None;
        }
        session.rx_buffer.clear();
        session.program_streaming = false;
        session.active_program_line = None;
        session.program_acknowledgements.clear();
        session.program_acknowledged_bytes = 0;
        session.continuous_jog = None;
        if was_connected {
            let _ = self.events.send(CncEvent::Disconnected {
                reason: "FluidNC WebSocket connection lost".into(),
            });
            self.publish_status(disconnected_status());
        }
    }

    fn trace_frame(_direction: &str, _opcode: &str, _payload: &[u8]) {}

    fn trace_program_event(event: &str, line: u32, _detail: &str) {
        if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
            static TRACE_START: OnceLock<Instant> = OnceLock::new();
            let elapsed_us = TRACE_START.get_or_init(Instant::now).elapsed().as_micros();
            eprintln!("[FluidNC] +{elapsed_us}us line={line} {event}");
        }
    }

    fn trace_binary_response(_payload: &[u8]) {}

    async fn send_text(&self, session: &mut Session, text: &str) -> CncResult<()> {
        // FluidNC завершает G-code-команду переводом строки. Например, кадр
        // WebUI "$G" длиной 3 байта содержит "$G\n".
        let payload = format!("{}\n", text.trim_end_matches(['\r', '\n']));
        Self::trace_frame("send", "text", payload.as_bytes());
        tracing::debug!(command = %text, "sending FluidNC line");
        let result = match session.socket.as_mut() {
            Some(socket) => timeout(self.config.ack_timeout, socket.send(Message::Text(payload)))
                .await
                .map_err(|_| {
                    ControllerError::new("timeout", "timed out sending command to FluidNC")
                })?
                .map_err(|error| ControllerError::new("io", error.to_string())),
            None => Err(ControllerError::new(
                "connection",
                "FluidNC WebSocket is disconnected",
            )),
        };

        if result.is_err() {
            self.invalidate_connection(session);
        }
        result
    }

    async fn send_realtime(&self, session: &mut Session, byte: u8) -> CncResult<()> {
        if byte != b'?' {
            Self::trace_frame("send", "binary", &[byte]);
        }
        let result = match session.socket.as_mut() {
            Some(socket) => timeout(
                self.config.ack_timeout,
                socket.send(Message::Binary(vec![byte])),
            )
            .await
            .map_err(|_| {
                ControllerError::new("timeout", "timed out sending realtime command to FluidNC")
            })?
            .map_err(|error| ControllerError::new("io", error.to_string())),
            None => Err(ControllerError::new(
                "connection",
                "FluidNC WebSocket is disconnected",
            )),
        };

        if result.is_err() {
            self.invalidate_connection(session);
        }
        result
    }

    fn extract_line(buffer: &mut Vec<u8>) -> CncResult<Option<String>> {
        let Some(end) = buffer.iter().position(|byte| *byte == b'\n') else {
            if buffer.len() > MAX_RESPONSE_LINE {
                return Err(ControllerError::new(
                    "protocol",
                    "FluidNC response line exceeds size limit",
                ));
            }
            return Ok(None);
        };

        if end > MAX_RESPONSE_LINE {
            return Err(ControllerError::new(
                "protocol",
                "FluidNC response line exceeds size limit",
            ));
        }

        let mut line: Vec<u8> = buffer.drain(..=end).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Ok(Some(String::from_utf8_lossy(&line).trim().to_owned()))
    }

    async fn read_line(session: &mut Session, duration: Duration) -> CncResult<String> {
        loop {
            if let Some(line) = Self::extract_line(&mut session.rx_buffer)? {
                return Ok(line);
            }

            let socket = session.socket.as_mut().ok_or_else(|| {
                ControllerError::new("connection", "FluidNC WebSocket is disconnected")
            })?;
            let message = timeout(duration, socket.next())
                .await
                .map_err(|_| {
                    ControllerError::new("timeout", "timed out waiting for FluidNC response")
                })?
                .ok_or_else(|| ControllerError::new("disconnected", "FluidNC WebSocket closed"))?
                .map_err(|error| ControllerError::new("io", error.to_string()))?;

            match message {
                Message::Text(text) => {
                    Self::trace_frame("recv", "text", text.as_bytes());
                    tracing::debug!(response = %text, "received FluidNC text frame");
                    let text = text.as_str();
                    if !text.starts_with("currentID:") && !text.starts_with("CURRENT_ID:") {
                        session.rx_buffer.extend_from_slice(text.as_bytes());
                    }
                }
                Message::Binary(bytes) => {
                    Self::trace_binary_response(&bytes);
                    session.rx_buffer.extend_from_slice(&bytes);
                }
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Close(_) => {
                    session.socket = None;
                    return Err(ControllerError::new(
                        "disconnected",
                        "FluidNC WebSocket closed",
                    ));
                }
                _ => {}
            }
        }
    }

    async fn send_command(&self, session: &mut Session, command: &str) -> CncResult<()> {
        if command.contains(['\n', '\r']) {
            return Err(ControllerError::new(
                "bad_command",
                "a FluidNC command must contain exactly one line",
            ));
        }
        if command.trim().is_empty() {
            return Ok(());
        }

        self.send_text(session, command.trim()).await?;
        let result = match timeout(self.config.ack_timeout, async {
            loop {
                let line = Self::read_line(session, self.config.ack_timeout).await?;
                if line == "ok" {
                    if let Some(system) = Self::coordinate_system_select(command) {
                        session.active_coordinate_system = system;
                    }
                    return Ok(());
                }
                if let Some(error) = line.strip_prefix("error:") {
                    tracing::warn!(controller_error = %error, "FluidNC rejected command");
                    return Err(ControllerError::new(
                        "controller",
                        format!("FluidNC rejected command ({error})"),
                    ));
                }
                if line.starts_with("ALARM:") {
                    tracing::error!(alarm = %line, "FluidNC alarm");
                    return Err(ControllerError::new("alarm", line));
                }
                // FluidNC may interleave status reports, startup banners and [GC:...] messages.
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC command acknowledgement",
            )),
        };

        // Only transport-level failures (timeouts, I/O, closed socket) drop the
        // session. A controller `error:N` or `ALARM:` reply rejects this one command
        // while the socket stays healthy, so the session is kept and the error is
        // returned to the caller.
        if result.as_ref().is_err_and(|error| {
            !matches!(error.code.as_str(), "gcode_error" | "alarm" | "controller")
        }) {
            self.invalidate_connection(session);
        }
        result
    }

    async fn query_status(&self, session: &mut Session) -> CncResult<String> {
        let started = Instant::now();
        let buffered_before = session.rx_buffer.len();
        self.send_realtime(session, b'?').await?;
        let result = match timeout(self.config.status_timeout, async {
            loop {
                let line = Self::read_line(session, self.config.status_timeout).await?;
                if line.starts_with('<') && line.ends_with('>') {
                    tracing::debug!(
                        elapsed_ms = started.elapsed().as_millis(),
                        buffered_before,
                        buffered_after = session.rx_buffer.len(),
                        response = %line,
                        "FluidNC status response received"
                    );
                    return Ok(line);
                }
                Self::record_program_response(session, &line);

                // Поддерживаем очередь команд заполненной, пока контроллер
                // подтверждает строки и в программе остаются команды.
                if session.program_streaming {
                    self.fill_program_window(session).await?;
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC status",
            )),
        };

        if let Err(error) = &result {
            if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
                eprintln!(
                    "[FluidNC WS] status query failed after {}ms (buffered_before={}): {}: {}; buffered={:?}",
                    started.elapsed().as_millis(),
                    buffered_before,
                    error.code,
                    error.message,
                    String::from_utf8_lossy(&session.rx_buffer),
                );
            }
            // A failed status query leaves unread/stale frames on the stream;
            // discard it so the next cycle performs fresh discovery/connect.
            // A single missed status reply is usually transient (busy controller,
            // delayed frame). Tolerate a bounded number of consecutive timeouts so
            // one late reply does not drop an active WebSocket session.
            let consecutive = self.status_timeouts.fetch_add(1, Ordering::SeqCst) + 1;
            if error.code == "timeout" && consecutive < STATUS_TIMEOUT_LIMIT {
                if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
                    eprintln!(
                        "[FluidNC WS] status timeout tolerated consecutive={consecutive} limit={STATUS_TIMEOUT_LIMIT}"
                    );
                }
                tracing::warn!(
                    consecutive,
                    limit = STATUS_TIMEOUT_LIMIT,
                    elapsed_ms = started.elapsed().as_millis(),
                    buffered_before,
                    "FluidNC status timeout tolerated; keeping WebSocket session"
                );
            } else {
                self.status_timeouts.store(0, Ordering::SeqCst);
                if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
                    eprintln!(
                        "[FluidNC WS] invalidating after status failure code={} consecutive={consecutive}",
                        error.code
                    );
                }
                tracing::warn!(
                    error_code = %error.code,
                    error = %error.message,
                    consecutive,
                    elapsed_ms = started.elapsed().as_millis(),
                    buffered_before,
                    buffered_after = session.rx_buffer.len(),
                    "invalidating FluidNC connection after status-query failure"
                );
                self.invalidate_connection(session);
            }
        }
        result
    }

    /// Sends one `$ESP` query and returns the value from its `$ESPnnn=value` line.
    async fn query_esp_setting(
        &self,
        session: &mut Session,
        command: &str,
    ) -> CncResult<Option<String>> {
        self.send_text(session, command).await?;
        let prefix = format!("{command}=");
        let mut value: Option<String> = None;
        let result = match timeout(self.config.ack_timeout, async {
            loop {
                let line = Self::read_line(session, self.config.ack_timeout).await?;
                if line == "ok" {
                    return Ok(());
                }
                if let Some(rest) = line.strip_prefix(&prefix) {
                    value = Some(rest.trim().to_string());
                }
                if let Some(error) = line.strip_prefix("error:") {
                    return Err(ControllerError::new(
                        "controller",
                        format!("FluidNC rejected {command} ({error})"),
                    ));
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC capability response",
            )),
        };
        result.map(|()| value)
    }

    /// Reads the other protocol settings of the connected FluidNC (`$ESP130/131`
    /// for Telnet, `$ESP120/121` for HTTP/WebSocket) instead of scanning the same
    /// host. Only values the controller actually reported become verified entries.
    async fn query_protocol_capabilities(
        &self,
        session: &mut Session,
        address: std::net::Ipv4Addr,
    ) {
        let mut values: HashMap<&'static str, String> = HashMap::new();
        for command in ["$ESP130", "$ESP131", "$ESP120", "$ESP121"] {
            match self.query_esp_setting(session, command).await {
                Ok(Some(value)) => {
                    tracing::info!(command, value = %value, "FluidNC capability read");
                    values.insert(command, value);
                }
                Ok(None) => {
                    tracing::warn!(command, "FluidNC capability response had no value");
                }
                Err(error) => {
                    tracing::warn!(command, ?error, "FluidNC capability query failed");
                    return;
                }
            }
        }

        let mut verified = Vec::new();
        let telnet_enabled = values.get("$ESP130").is_some_and(|value| value == "ON");
        let telnet_port = values
            .get("$ESP131")
            .and_then(|value| value.parse::<u16>().ok());
        if let (true, Some(port)) = (telnet_enabled, telnet_port) {
            verified.push(DiscoveredDevice {
                id: format!("fluidnc:Telnet:{address}:{port}"),
                protocol: "FluidNC".into(),
                machine_name: None,
                version: None,
                transport: DiscoveryTransport::Telnet,
                endpoint: format!("{address}:{port}"),
            });
        }
        // FluidNC serves HTTP and WebSocket on the same port, so $ESP120/$ESP121
        // verify the WebSocket entry as well.
        let http_enabled = values.get("$ESP120").is_some_and(|value| value == "ON");
        let http_port = values
            .get("$ESP121")
            .and_then(|value| value.parse::<u16>().ok());
        if let (true, Some(port)) = (http_enabled, http_port) {
            verified.push(DiscoveredDevice {
                id: format!("fluidnc:WebSocket:{address}:{port}"),
                protocol: "FluidNC".into(),
                machine_name: None,
                version: None,
                transport: DiscoveryTransport::WebSocket,
                endpoint: format!("{address}:{port}"),
            });
        }
        let host = address.to_string();
        if let Ok(mut table) = self.verified_capabilities.write() {
            table.retain(|device| {
                device
                    .endpoint
                    .rsplit_once(':')
                    .is_none_or(|(device_host, _)| device_host != host)
            });
            table.extend(verified);
        }
    }

    async fn query_settings(&self, session: &mut Session) -> CncResult<HashMap<u16, f64>> {
        self.send_text(session, "$$").await?;

        let result = match timeout(self.config.ack_timeout, async {
            let mut settings = HashMap::new();
            loop {
                let line = Self::read_line(session, self.config.ack_timeout).await?;

                if line == "ok" {
                    return Ok(settings);
                }
                if let Some(error) = line.strip_prefix("error:") {
                    return Err(ControllerError::new("settings", error));
                }
                if line.starts_with("ALARM:") {
                    return Err(ControllerError::new("alarm", line));
                }

                let Some((number, value)) = line
                    .strip_prefix('$')
                    .and_then(|setting| setting.split_once('='))
                else {
                    continue;
                };
                let (Ok(number), Ok(value)) = (number.parse::<u16>(), value.parse::<f64>()) else {
                    continue;
                };
                if value.is_finite() {
                    settings.insert(number, value);
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC settings",
            )),
        };

        if result
            .as_ref()
            .is_err_and(|error| error.code != "settings" && error.code != "alarm")
        {
            self.invalidate_connection(session);
        }
        result
    }

    fn reported_axis_count(response: &str) -> Option<usize> {
        response
            .split('|')
            .find(|field| field.starts_with("MPos:") || field.starts_with("WPos:"))
            .and_then(|field| field.split_once(':'))
            .map(|(_, values)| values.split(',').count())
            .filter(|count| (3..=6).contains(count))
    }

    fn parse_axes(value: &str) -> Option<[f64; 6]> {
        let mut axes = [0.0_f64; 6];
        let mut count = 0;

        for component in value.split(',') {
            if count == axes.len() {
                return None;
            }
            axes[count] = component.parse().ok()?;
            if !axes[count].is_finite() {
                return None;
            }
            count += 1;
        }

        (count >= 3).then_some(axes)
    }

    fn parse_status(session: &Session, response: &str) -> CncResult<MachineStatus> {
        let body = response
            .strip_prefix('<')
            .and_then(|value| value.strip_suffix('>'))
            .ok_or_else(|| ControllerError::new("protocol", "malformed FluidNC status response"))?;

        let mut fields = body.split('|');
        let state = fields.next().unwrap_or("Unknown");
        let mut machine_position: Option<[f64; 6]> = None;
        let mut work_position: Option<[f64; 6]> = None;
        let mut work_offset: Option<[f64; 6]> = None;
        let mut feed_rate = 0.0;
        let mut spindle_rpm = 0.0;

        for field in fields {
            if let Some(value) = field.strip_prefix("MPos:") {
                machine_position = Self::parse_axes(value);
            } else if let Some(value) = field.strip_prefix("WPos:") {
                work_position = Self::parse_axes(value);
            } else if let Some(value) = field.strip_prefix("WCO:") {
                work_offset = Self::parse_axes(value);
            } else if let Some(value) = field.strip_prefix("FS:") {
                let mut values = value.split(',');
                feed_rate = values
                    .next()
                    .and_then(|item| item.parse().ok())
                    .unwrap_or(0.0);
                spindle_rpm = values
                    .next()
                    .and_then(|item| item.parse().ok())
                    .unwrap_or(0.0);
            } else if let Some(value) = field.strip_prefix("F:") {
                feed_rate = value.parse().unwrap_or(0.0);
            }
        }

        let offset = work_offset
            .or_else(|| match (machine_position, work_position) {
                (Some(machine), Some(work)) => {
                    Some(std::array::from_fn(|axis| machine[axis] - work[axis]))
                }
                _ => None,
            })
            .or(session.work_offset)
            .unwrap_or([0.0; 6]);
        let machine = machine_position
            .or_else(|| {
                work_position.map(|work| std::array::from_fn(|axis| work[axis] + offset[axis]))
            })
            .ok_or_else(|| {
                ControllerError::new("protocol", "FluidNC status contains no valid position")
            })?;

        let lower_state = state.to_ascii_lowercase();
        let alarm = lower_state.starts_with("alarm");
        let running = lower_state == "run";
        let jogging = lower_state == "jog";
        let paused = lower_state.starts_with("hold") || lower_state.starts_with("door");

        Ok(MachineStatus {
            connected: true,
            task_mode: if running {
                TaskMode::Auto
            } else {
                TaskMode::Manual
            },
            task_state: if alarm {
                TaskState::Estop
            } else {
                TaskState::On
            },
            interpreter_state: if running {
                InterpreterState::Reading
            } else if paused {
                InterpreterState::Paused
            } else {
                InterpreterState::Idle
            },
            position: Position {
                x: machine[0],
                y: machine[1],
                z: machine[2],
                a: machine[3],
                b: machine[4],
                c: machine[5],
            },
            work_offset: Position {
                x: offset[0],
                y: offset[1],
                z: offset[2],
                a: offset[3],
                b: offset[4],
                c: offset[5],
            },
            active_coordinate_system: session.active_coordinate_system,
            coordinate_systems: Vec::new(),
            feed_rate: if jogging { 0.0 } else { feed_rate },
            spindle: SpindleStatus {
                commanded_rpm: session.spindle_command.rpm,
                actual_rpm: spindle_rpm,
                direction: session.spindle_command.direction,
                enabled: spindle_rpm > 0.0 || session.spindle_command.rpm > 0.0,
            },
            mist_coolant_on: false,
            flood_coolant_on: false,
            jog_rate: if jogging { feed_rate } else { 0.0 },
            motion_mode: if jogging {
                Some("Jog".into())
            } else if running {
                Some("G-code".into())
            } else {
                None
            },
            max_spindle_rpm: 0.0,
            loaded_program: session.program_name.clone(),
            current_line: session.current_line,
            total_lines: session.program.len() as u32,
            message: Some(format!("FluidNC {state}")),
            homed: [false; 6],
            homing: lower_state == "home",
        })
    }

    fn coordinate_number(system: CoordinateSystem) -> u8 {
        match system {
            CoordinateSystem::G54 => 1,
            CoordinateSystem::G55 => 2,
            CoordinateSystem::G56 => 3,
            CoordinateSystem::G57 => 4,
            CoordinateSystem::G58 => 5,
            CoordinateSystem::G59 => 6,
            CoordinateSystem::G59_1 => 7,
            CoordinateSystem::G59_2 => 8,
            CoordinateSystem::G59_3 => 9,
        }
    }

    fn axis_letter(axis: Axis) -> char {
        match axis {
            Axis::X => 'X',
            Axis::Y => 'Y',
            Axis::Z => 'Z',
            Axis::A => 'A',
            Axis::B => 'B',
            Axis::C => 'C',
        }
    }

    fn coordinate_system_select(command: &str) -> Option<CoordinateSystem> {
        let command = command.to_ascii_uppercase();
        CoordinateSystem::ALL.into_iter().find(|system| {
            command
                .split_whitespace()
                .any(|word| word == system.label())
        })
    }

    fn program_line_wire_bytes(line: &str) -> usize {
        line.len().saturating_add(1)
    }

    async fn send_next_program_line(
        &self,
        session: &mut Session,
        config: &FluidNcWebSocketConfig,
    ) -> CncResult<()> {
        if !session.program_streaming {
            return Ok(());
        }

        let index = session.current_line as usize;
        if index >= session.program.len() {
            return Ok(());
        }

        let line = session.program[index].clone();
        let wire_bytes = Self::program_line_wire_bytes(&line);
        if line.len() > config.max_line_len {
            return Err(ControllerError::new(
                "protocol",
                format!(
                    "FluidNC program line {} exceeds configured maximum length",
                    index + 1
                ),
            ));
        }
        if session
            .program_acknowledged_bytes
            .saturating_add(wire_bytes)
            > config.rx_buffer_bytes
        {
            return Ok(());
        }
        if let Err(error) = self.send_text(session, &line).await {
            session.program_streaming = false;
            session.active_program_line = None;
            session.program_acknowledgements.clear();
            session.program_acknowledged_bytes = 0;
            session.program_error = Some(format!("{}: {}", error.code, error.message));
            return Err(error);
        }
        Self::trace_program_event("sent", index as u32 + 1, "program line sent");

        session
            .program_acknowledgements
            .push_back((index as u32, wire_bytes));
        session.program_acknowledged_bytes = session
            .program_acknowledged_bytes
            .saturating_add(wire_bytes);
        session.active_program_line = session
            .program_acknowledgements
            .front()
            .map(|(line, _)| *line);
        session.current_line = (index + 1) as u32;
        Ok(())
    }

    async fn fill_program_window(&self, session: &mut Session) -> CncResult<()> {
        while session.program_streaming
            && (session.current_line as usize) < session.program.len()
            && session.program_acknowledged_bytes < self.config.rx_buffer_bytes
        {
            let line_before = session.current_line;
            self.send_next_program_line(session, &self.config).await?;
            if session.current_line == line_before {
                break;
            }
        }
        Ok(())
    }

    fn record_program_response(session: &mut Session, line: &str) {
        if session.program_acknowledgements.is_empty() {
            return;
        }

        if line == "ok" {
            if let Some((program_line, byte_count)) = session.program_acknowledgements.pop_front() {
                session.program_acknowledged_bytes = session
                    .program_acknowledged_bytes
                    .saturating_sub(byte_count);
                Self::trace_program_event("ack", program_line + 1, "response=ok");
            }
            session.active_program_line = session
                .program_acknowledgements
                .front()
                .map(|(line, _)| *line);
        } else if let Some(error) = line.strip_prefix("error:") {
            if let Some((program_line, _)) = session.program_acknowledgements.front() {
                Self::trace_program_event(
                    "rejected",
                    *program_line + 1,
                    "controller rejected program line",
                );
            }
            session.program_error =
                Some(format!("FluidNC отклонил строку программы: error:{error}"));
            session.program_streaming = false;
            session.active_program_line = None;
            session.program_acknowledgements.clear();
            session.program_acknowledged_bytes = 0;
        } else if line.starts_with("ALARM:") {
            session.program_error = Some(format!("FluidNC сообщил аварию: {line}"));
            session.program_streaming = false;
            session.active_program_line = None;
            session.program_acknowledgements.clear();
            session.program_acknowledged_bytes = 0;
        }
    }

    async fn send_jog_chunk(&self, session: &mut Session, jog: ContinuousJog) -> CncResult<()> {
        let command = if let Some(axis) = jog.axis {
            format!(
                "$J=G21 G91 {}{:.3} F{:.0}",
                Self::axis_letter(axis),
                JOG_CHUNK_MM * f64::from(jog.x),
                jog.feed_rate,
            )
        } else {
            let norm = f64::from(jog.x * jog.x + jog.y * jog.y).sqrt();
            let x = JOG_CHUNK_MM * f64::from(jog.x) / norm;
            let y = JOG_CHUNK_MM * f64::from(jog.y) / norm;
            format!("$J=G21 G91 X{x:.3} Y{y:.3} F{:.0}", jog.feed_rate)
        };
        self.send_command(session, &command).await
    }
}

#[async_trait::async_trait]
impl CncApi for FluidNcWebSocketAdapter {
    fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        FluidNcWebSocketAdapter::discovered_devices(self)
    }

    async fn rediscover_devices_stream(
        &self,
        priority_targets: Vec<String>,
    ) -> CncResult<tokio::sync::mpsc::Receiver<DiscoveredDevice>> {
        if !self.discovery_enabled {
            let (sender, receiver) = tokio::sync::mpsc::channel(16);
            for device in self.discovered_devices() {
                let _ = sender.send(device).await;
            }
            return Ok(receiver);
        }

        // While a session is active, no probe is sent: the connected controller
        // must not be loaded by scans. Only the cached table is returned.
        if self.connected_device_id().is_some() {
            tracing::info!("FluidNC rediscovery skipped: a session is active");
            let (sender, receiver) = tokio::sync::mpsc::channel(16);
            for device in self.discovered_devices() {
                let _ = sender.send(device).await;
            }
            return Ok(receiver);
        }

        // Keep the first auto-selected machine out of all rediscovery probes.
        // It remains excluded even after its connection is lost.
        tracing::debug!(
            connected_device = ?self.connected_device_id(),
            selected_device = ?self.selected_device_id.read().ok().and_then(|id| id.clone()),
            priority_count = priority_targets.len(),
            "starting manual FluidNC rediscovery"
        );
        let excluded_hosts: Vec<std::net::Ipv4Addr> = self
            .selected_device_id
            .read()
            .ok()
            .and_then(|id| id.as_deref().and_then(device_id_host))
            .into_iter()
            .collect();
        let discoveries = discover_fluidnc_stream(&FluidNcDiscoveryConfig {
            explicit_targets: self.config.discovery_port.clone(),
            priority_targets: {
                let mut cached = self
                    .priority_targets
                    .read()
                    .map(|targets| targets.clone())
                    .unwrap_or_default();
                cached.extend(priority_targets);
                cached
            },
            excluded_hosts,
            websocket_port: FLUIDNC_PORT,
            telnet_port: 23,
        })
        .await?;

        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        let devices = self.discovered_devices.clone();
        tokio::spawn(async move {
            let mut discoveries = discoveries;
            while let Some(device) = discoveries.recv().await {
                if let Ok(mut current) = devices.write() {
                    if !current.iter().any(|existing| existing.id == device.id) {
                        current.push(device.clone());
                    }
                }
                if sender.send(device).await.is_err() {
                    // No UI listener remains; continue consuming the scanner channel
                    // so probing completes and the scanner never stalls on a full buffer.
                    while discoveries.recv().await.is_some() {}
                    break;
                }
            }
        });
        Ok(receiver)
    }

    async fn rediscover_devices(&self) -> CncResult<Vec<DiscoveredDevice>> {
        let mut discoveries = self.rediscover_devices_stream(Vec::new()).await?;
        let mut devices = Vec::new();
        while let Some(device) = discoveries.recv().await {
            devices.push(device);
        }
        Ok(devices)
    }

    fn connected_device_id(&self) -> Option<String> {
        FluidNcWebSocketAdapter::connected_device_id(self)
    }

    async fn status(&self) -> CncResult<MachineStatus> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let response = self.query_status(&mut session).await?;
        self.status_timeouts.store(0, Ordering::SeqCst);

        let mut status = Self::parse_status(&session, &response)?;
        if let Some(count) = Self::reported_axis_count(&response) {
            session.reported_axis_count = count;
        }

        let machine_state = response
            .strip_prefix('<')
            .and_then(|value| value.split('|').next())
            .unwrap_or_default();
        let idle = machine_state.eq_ignore_ascii_case("Idle");

        let was_program_streaming = session.program_streaming;
        if session.program_streaming
            && idle
            && session.current_line as usize >= session.program.len()
            && session.program_acknowledgements.is_empty()
        {
            session.program_streaming = false;
        } else if idle && !session.program_streaming {
            if let Some(jog) = session.continuous_jog {
                self.send_jog_chunk(&mut session, jog).await?;
            }
        }

        if let Some(error) = session.program_error.take() {
            status.message = Some(error);
        }
        status.current_line = session
            .active_program_line
            .map_or(session.current_line, |line| line + 1);
        if session.active_program_line.is_some() {
            status.task_mode = TaskMode::Auto;
            if status.interpreter_state == InterpreterState::Idle {
                status.interpreter_state = InterpreterState::Reading;
            }
        }

        if response.contains("|WCO:")
            || (response.contains("|MPos:") && response.contains("|WPos:"))
        {
            session.work_offset = Some([
                status.work_offset.x,
                status.work_offset.y,
                status.work_offset.z,
                status.work_offset.a,
                status.work_offset.b,
                status.work_offset.c,
            ]);
        }
        if was_program_streaming && !session.program_streaming {
            let _ = self.events.send(CncEvent::ProgramFinished);
        }
        self.publish_status(status.clone());
        Ok(status)
    }

    fn last_status(&self) -> Option<MachineStatus> {
        Some(self.status_tx.borrow().clone())
    }

    fn subscribe_status(&self) -> Option<watch::Receiver<MachineStatus>> {
        Some(self.status_tx.subscribe())
    }

    fn subscribe_events(&self) -> Option<broadcast::Receiver<CncEvent>> {
        Some(self.events.subscribe())
    }

    async fn load_program_source(&self, source: ProgramSource) -> CncResult<()> {
        match source {
            ProgramSource::Path(path) => self.load_program(&path).await,
            ProgramSource::Inline(content) => {
                if content.len() > MAX_PROGRAM_BYTES {
                    return Err(ControllerError::new(
                        "bad_program",
                        format!("FluidNC program exceeds the {MAX_PROGRAM_BYTES}-byte size limit"),
                    ));
                }
                let lines = normalized_program_lines(&content);
                for (index, line) in lines.iter().enumerate() {
                    if line.len() > self.config.max_line_len {
                        return Err(ControllerError::new(
                            "bad_program",
                            format!(
                                "FluidNC program line {} exceeds configured maximum length",
                                index + 1
                            ),
                        ));
                    }
                }
                let mut session = self.session.lock().await;
                session.program = lines;
                session.program_name = Some("inline.gcode".into());
                session.current_line = 0;
                session.active_program_line = None;
                session.program_acknowledgements.clear();
                session.program_acknowledged_bytes = 0;
                session.program_error = None;
                session.program_streaming = false;
                Ok(())
            }
        }
    }

    async fn set_override(&self, kind: OverrideKind, value: f64) -> CncResult<()> {
        let _ = (kind, value);
        Err(ControllerError::new(
            "unsupported",
            "FluidNC override commands are not enabled until their controller-specific behavior is verified",
        ))
    }

    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;

        let status = self.query_status(&mut session).await?;
        if let Some(count) = Self::reported_axis_count(&status) {
            session.reported_axis_count = count;
        }

        // FluidNC exposes GRBL-compatible travel and spindle limits through $$:
        // $130-$135 are the maximum travels for X/Y/Z/A/B/C, $30/$31 spindle RPM.
        let settings = self.query_settings(&mut session).await?;
        let axis_definitions = [
            (MachineAxis::X, AxisUnits::Linear),
            (MachineAxis::Y, AxisUnits::Linear),
            (MachineAxis::Z, AxisUnits::Linear),
            (MachineAxis::A, AxisUnits::Angular),
            (MachineAxis::B, AxisUnits::Angular),
            (MachineAxis::C, AxisUnits::Angular),
        ];
        let axes = axis_definitions
            .into_iter()
            .enumerate()
            .take(session.reported_axis_count)
            .map(|(index, (axis, units))| AxisConfiguration {
                axis,
                units,
                max_travel: settings
                    .get(&(130 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
                max_velocity: settings
                    .get(&(110 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
                acceleration: settings
                    .get(&(120 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
                steps_per_unit: settings
                    .get(&(100 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
            })
            .collect();

        let min_rpm = settings.get(&31).copied().filter(|value| *value >= 0.0);
        let max_rpm = settings
            .get(&30)
            .copied()
            .filter(|value| *value > 0.0)
            .filter(|maximum| min_rpm.is_none_or(|minimum| minimum <= *maximum));

        Ok(MachineConfiguration {
            axes,
            spindle: SpindleConfiguration { min_rpm, max_rpm },
            settings,
        })
    }

    async fn power_on(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, "$X").await
    }

    async fn power_off(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "FluidNC has no safe software power-off command; use the machine hardware controls",
        ))
    }

    async fn estop(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "WebSocket commands are not an emergency stop; use the machine hardware E-Stop",
        ))
    }

    async fn home(&self, axis: Option<Axis>) -> CncResult<()> {
        if axis.is_some() {
            return Err(ControllerError::new(
                "unsupported",
                "FluidNC $H homes all configured axes; individual-axis homing is unsupported",
            ));
        }
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, "$H").await
    }

    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, system.label()).await?;
        session.active_coordinate_system = system;
        Ok(())
    }

    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let axes = ['X', 'Y', 'Z', 'A', 'B', 'C']
            .into_iter()
            .take(session.reported_axis_count)
            .map(|axis| format!("{axis}0"))
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!("G10 L20 P{} {axes}", Self::coordinate_number(system));
        self.send_command(&mut session, &command).await?;
        session.work_offset = None;
        Ok(())
    }

    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let command = format!(
            "G10 L20 P{} {}0",
            Self::coordinate_number(session.active_coordinate_system),
            Self::axis_letter(axis),
        );
        self.send_command(&mut session, &command).await?;
        session.work_offset = None;
        Ok(())
    }

    async fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        if ![-1, 1].contains(&request.direction)
            || !request.distance.is_finite()
            || request.distance <= 0.0
        {
            return Err(ControllerError::new(
                "bad_jog",
                "invalid FluidNC jog request",
            ));
        }
        let feed_rate = request.feed_rate.unwrap_or(100.0);
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }

        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let command = format!(
            "$J=G21 G91 {}{:.4} F{feed_rate}",
            Self::axis_letter(request.axis),
            request.distance * f64::from(request.direction),
        );
        self.send_command(&mut session, &command).await
    }

    async fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()> {
        if ![-1, 0, 1].contains(&direction) {
            return Err(ControllerError::new(
                "bad_jog",
                "invalid continuous jog direction",
            ));
        }
        if direction == 0 {
            return self.cancel_jog().await;
        }
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }

        let jog = ContinuousJog {
            x: direction,
            y: 0,
            axis: Some(axis),
            feed_rate,
        };
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.continuous_jog = Some(jog);
        self.send_jog_chunk(&mut session, jog).await
    }

    async fn jog_continuous_xy(
        &self,
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        if ![-1, 0, 1].contains(&x_direction)
            || ![-1, 0, 1].contains(&y_direction)
            || !feed_rate.is_finite()
            || (x_direction != 0 || y_direction != 0) && feed_rate <= 0.0
        {
            return Err(ControllerError::new("bad_jog", "invalid continuous XY jog"));
        }
        if x_direction == 0 && y_direction == 0 {
            return self.cancel_jog().await;
        }

        let jog = ContinuousJog {
            x: x_direction,
            y: y_direction,
            axis: None,
            feed_rate,
        };
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.continuous_jog = Some(jog);
        self.send_jog_chunk(&mut session, jog).await
    }

    async fn cancel_jog(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.continuous_jog = None;
        self.send_realtime(&mut session, 0x85).await
    }

    async fn stop_motion(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let jogging = session.continuous_jog.take().is_some();
        self.send_realtime(&mut session, if jogging { 0x85 } else { b'!' })
            .await
    }

    async fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, command).await
    }

    async fn load_program(&self, path: &Path) -> CncResult<()> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        if content.len() > MAX_PROGRAM_BYTES {
            return Err(ControllerError::new(
                "bad_program",
                format!("FluidNC program exceeds the {MAX_PROGRAM_BYTES}-byte size limit"),
            ));
        }

        let lines = normalized_program_lines(&content);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("program.nc")
            .to_owned();

        let mut session = self.session.lock().await;
        session.program = lines;
        session.program_name = Some(name);
        session.current_line = 0;
        session.active_program_line = None;
        session.program_acknowledgements.clear();
        session.program_acknowledged_bytes = 0;
        session.program_error = None;
        session.program_streaming = false;
        Ok(())
    }

    async fn program_run(&self, start_line: u32) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let status = self.query_status(&mut session).await?;
        let machine_state = status
            .strip_prefix('<')
            .and_then(|value| value.split('|').next())
            .unwrap_or("");
        if !machine_state.eq_ignore_ascii_case("Idle") {
            return Err(ControllerError::new(
                "motion_busy",
                format!("cannot start a FluidNC program while state is {machine_state}"),
            ));
        }

        let start = start_line as usize;
        if session.program.is_empty() || start >= session.program.len() {
            return Err(ControllerError::new(
                "bad_program",
                "no program loaded or start line is outside the loaded FluidNC program",
            ));
        }

        session.current_line = start_line;
        session.active_program_line = None;
        session.program_acknowledgements.clear();
        session.program_acknowledged_bytes = 0;
        session.program_error = None;
        session.program_streaming = true;
        self.fill_program_window(&mut session).await?;
        let _ = self.events.send(CncEvent::ProgramStarted);
        Ok(())
    }

    async fn program_pause(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_realtime(&mut session, b'!').await
    }

    async fn program_resume(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_realtime(&mut session, b'~').await
    }

    async fn program_abort(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.program_streaming = false;
        session.continuous_jog = None;
        session.current_line = 0;
        session.active_program_line = None;
        session.program_acknowledgements.clear();
        session.program_acknowledged_bytes = 0;
        // FluidNC сообщает ALARM:3 (Abort Cycle) после Ctrl-X. Это ожидаемый
        // ответ на Stop, но контроллер всё равно может остаться в состоянии
        // Alarm; не следует подавлять его или считать, что $X уже выполнен.
        self.send_realtime(&mut session, 0x18).await
    }

    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        if !command.rpm.is_finite() || command.rpm < 0.0 {
            return Err(ControllerError::new(
                "bad_spindle",
                "spindle RPM must be finite and non-negative",
            ));
        }

        let (gcode, normalized) = match command.direction {
            _ if command.rpm == 0.0 => (
                "M5".to_owned(),
                SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
            ),
            SpindleDirection::Stopped => (
                "M5".to_owned(),
                SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
            ),
            SpindleDirection::Clockwise => (format!("M3 S{}", command.rpm), command),
            SpindleDirection::CounterClockwise => (format!("M4 S{}", command.rpm), command),
        };

        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, &gcode).await?;
        session.spindle_command = normalized;
        Ok(())
    }

    async fn clear_error(&self) -> CncResult<()> {
        self.power_on().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_api_starts_with_disconnected_snapshot_and_subscriptions() {
        let adapter = FluidNcWebSocketAdapter::connect("ws://127.0.0.1:81/")
            .expect("valid WebSocket URL should construct an adapter");

        let snapshot = adapter.last_status().expect("status snapshot is supported");
        assert!(!snapshot.connected);
        assert_eq!(machine_state(&snapshot), MachineState::Disconnected);
        assert!(adapter.subscribe_status().is_some());
        assert!(adapter.subscribe_events().is_some());
    }

    #[test]
    fn program_normalization_matches_browser_executable_line_counting() {
        let source = concat!(
            "; full-line comment\n",
            "(parenthesized comment)\n",
            "G21 (inline comment) ; trailing comment\n",
            "  G1 X10 (more) Y20  ; motion comment\n",
            "(comment only); trailing comment\n",
            "M5\n",
        );

        assert_eq!(
            normalized_program_lines(source),
            ["G21", "G1 X10  Y20", "M5"]
        );
    }

    #[test]
    fn program_character_count_uses_utf8_wire_bytes_and_newline() {
        assert_eq!(FluidNcWebSocketAdapter::program_line_wire_bytes("G1 X1"), 6);
        assert_eq!(
            FluidNcWebSocketAdapter::program_line_wire_bytes("G1 (µ)"),
            "G1 (µ)".len() + 1
        );
    }

    #[test]
    fn reads_axis_count_from_fluidnc_status_fields() {
        assert_eq!(
            FluidNcWebSocketAdapter::reported_axis_count("<Idle|MPos:1,2,3,4,5,6|FS:0,0>"),
            Some(6)
        );
        assert_eq!(
            FluidNcWebSocketAdapter::reported_axis_count("<Idle|WPos:1,2,3|FS:0,0>"),
            Some(3)
        );
        assert_eq!(
            FluidNcWebSocketAdapter::reported_axis_count("<Idle|FS:0,0>"),
            None
        );
    }
}
