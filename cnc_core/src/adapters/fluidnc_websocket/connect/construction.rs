//! Adapter constructors and initial session/watch-channel setup.

use super::super::{
    disconnected_status, FluidNcWebSocketAdapter, FluidNcWebSocketConfig, Session,
    TelnetParseState, EVENT_CAPACITY, FLUIDNC_PORT,
};
use crate::{CncResult, ControllerError, CoordinateSystem, SpindleCommand, SpindleDirection};
use std::{
    collections::VecDeque,
    sync::{atomic::AtomicBool, Arc, RwLock},
};
use tokio::sync::{broadcast, watch, Mutex};

/// Builds the cached WebSocket URL only for a confirmed/default WebSocket port.
/// Telnet endpoints are deliberately rejected instead of being remapped to port 80.
pub(in crate::adapters::fluidnc_websocket) fn cached_websocket_url(
    endpoint: &str,
) -> Option<String> {
    let (host, port) = endpoint.rsplit_once(':')?;
    if port.parse::<u16>().ok()? != FLUIDNC_PORT {
        return None;
    }
    let address = host.parse::<std::net::Ipv4Addr>().ok()?;
    Some(format!("ws://{address}:{FLUIDNC_PORT}/"))
}

impl FluidNcWebSocketAdapter {
    /// Create a FluidNC adapter that controls the machine over its Telnet endpoint.
    pub fn connect_telnet(endpoint: impl Into<String>) -> CncResult<Self> {
        let endpoint = endpoint.into();
        let (host, port) = endpoint.rsplit_once(':').ok_or_else(|| {
            ControllerError::new("connect", "FluidNC Telnet endpoint must be host:port")
        })?;
        if host.is_empty() || port.parse::<u16>().ok().filter(|port| *port != 0).is_none() {
            return Err(ControllerError::new(
                "connect",
                "FluidNC Telnet endpoint must contain a valid host and port",
            ));
        }
        // The Telnet endpoint is pinned and handled before discovery in
        // `ensure_connected`; discovery-enabled construction permits a missing
        // WebSocket URL without enabling a WebSocket fallback for this adapter.
        let mut adapter = Self::construct(None, FluidNcWebSocketConfig::default(), true)?;
        adapter.telnet_endpoint = Some(endpoint);
        Ok(adapter)
    }

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
    pub(super) fn construct(
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
            telnet_endpoint: None,
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
                telnet_parse_state: TelnetParseState::Data,
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
}
