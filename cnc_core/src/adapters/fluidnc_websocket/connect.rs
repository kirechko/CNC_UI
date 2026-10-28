//! Construction, status publication, discovery-table bookkeeping and the
//! connection lifecycle (auto connect, priority fast path, invalidation)
//! of [`FluidNcWebSocketAdapter`]. The single `CncApi` trait implementation
//! lives in `api.rs`; everything here is an inherent helper block.

use super::{
    device_id_host, disconnected_status, FluidNcWebSocketAdapter, FluidNcWebSocketConfig, Session,
    EVENT_CAPACITY, FLUIDNC_PORT,
};
use crate::discovery::{
    discover_fluidnc_stream, fluidnc_websocket_address, DiscoveredDevice, DiscoveryTransport,
    FluidNcDiscoveryConfig,
};
use crate::{
    machine_state, ControllerError, CncEvent, CncResult, CoordinateSystem, MachineState,
    MachineStatus, SpindleCommand, SpindleDirection,
};
use std::{
    collections::VecDeque,
    sync::{atomic::AtomicBool, Arc, RwLock},
};
use tokio::sync::{broadcast, mpsc, watch, Mutex};
use tokio_tungstenite::connect_async;

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

    /// Shared constructor: validates the runtime configuration and seeds the
    /// disconnected session/watch state for both direct-URL and discovery modes.
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
                toolpath: crate::Toolpath::default(),
                last_progress: None,
            }),
            events,
            status_tx,
        })
    }

    /// Push a status snapshot into the watch channel and translate meaningful
    /// diffs (state, program line, alarm entry) into broadcast events.
    pub(super) fn publish_status(&self, status: MachineStatus) {
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

    /// Cached discovery table for the UI. Entries of the auto-selected host are
    /// hidden (that machine is already attached), but verified cross-protocol
    /// capabilities of the connected controller are shown instead of scan rows.
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

    /// Identifier of the controller the adapter is currently attached to.
    pub fn connected_device_id(&self) -> Option<String> {
        self.connected_device_id
            .read()
            .ok()
            .and_then(|id| id.clone())
    }

    /// Bring the socket up if it is down. Direct-URL adapters dial the fixed
    /// endpoint; discovery adapters first retry cached priority targets (fast
    /// path) and only then run a fresh scan, taking the first confirmed
    /// WebSocket. Auto-connected sessions are not re-dialed after a loss.
    pub(super) async fn ensure_connected(&self, session: &mut Session) -> CncResult<()> {
        if session.socket.is_some() {
            return Ok(());
        }

        if self.discovery_enabled && self.auto_connected.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ControllerError::new(
                "disconnected",
                "FluidNC connection was lost; reconnect manually",
            ));
        }

        if !self.discovery_enabled {
            let url = self.url.clone().ok_or_else(|| {
                ControllerError::new("connect", "FluidNC WebSocket URL is not configured")
            })?;
            let (socket, _) = tokio::time::timeout(self.config.connect_timeout, connect_async(&url))
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
            match tokio::time::timeout(self.config.connect_timeout, connect_async(&url)).await {
                Ok(Ok((socket, _))) => {
                    session.socket = Some(socket);
                    self.auto_connected
                        .store(true, std::sync::atomic::Ordering::SeqCst);
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
            match tokio::time::timeout(self.config.connect_timeout, connect_async(&url)).await {
                Ok(Ok((socket, _))) => {
                    session.socket = Some(socket);
                    self.auto_connected
                        .store(true, std::sync::atomic::Ordering::SeqCst);
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

    /// Forget the socket and all transport state, publishing a disconnected
    /// snapshot when there was an active session to lose.
    #[track_caller]
    pub(super) fn invalidate_connection(&self, session: &mut Session) {
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

    /// Start a manual rediscovery scan and stream results through a channel.
    /// The connected host stays excluded; while a session is active no probes
    /// are sent at all and only the cached table is replayed.
    pub(super) async fn start_rediscovery_stream(
        &self,
        priority_targets: Vec<String>,
    ) -> CncResult<mpsc::Receiver<DiscoveredDevice>> {
        if !self.discovery_enabled {
            let (sender, receiver) = mpsc::channel(16);
            for device in self.discovered_devices() {
                let _ = sender.send(device).await;
            }
            return Ok(receiver);
        }

        // While a session is active, no probe is sent: the connected controller
        // must not be loaded by scans. Only the cached table is returned.
        if self.connected_device_id().is_some() {
            tracing::info!("FluidNC rediscovery skipped: a session is active");
            let (sender, receiver) = mpsc::channel(16);
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

        let (sender, receiver) = mpsc::channel(64);
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
}
