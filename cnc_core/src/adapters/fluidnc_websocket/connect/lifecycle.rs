//! Connection establishment, including pinned transports and discovery fast paths.

use super::super::{
    FluidNcConnection, FluidNcWebSocketAdapter, Session, TelnetParseState, FLUIDNC_PORT,
};
use super::cached_websocket_url;
use crate::discovery::{
    discover_fluidnc_stream, fluidnc_websocket_address, DiscoveryTransport, FluidNcDiscoveryConfig,
};
use crate::{CncEvent, CncResult, ControllerError};
use tokio_tungstenite::connect_async;

impl FluidNcWebSocketAdapter {
    /// Bring the selected transport up if it is down. Direct-URL adapters dial the fixed
    /// endpoint; discovery adapters first retry cached priority targets (fast
    /// path) and only then run a fresh scan, taking the first confirmed
    /// WebSocket. Auto-connected sessions are not re-dialed after a loss.
    pub(in crate::adapters::fluidnc_websocket) async fn ensure_connected(
        &self,
        session: &mut Session,
    ) -> CncResult<()> {
        if session.socket.is_some() {
            return Ok(());
        }

        if let Some(endpoint) = &self.telnet_endpoint {
            // Explicit transport selection is a pin: reconnect only to that
            // Telnet endpoint, and never fall through to WebSocket discovery.
            if self
                .auto_connected
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                return Err(ControllerError::new(
                    "disconnected",
                    "FluidNC Telnet connection was lost; reconnect manually",
                ));
            }
            let stream = tokio::time::timeout(
                self.config.connect_timeout,
                tokio::net::TcpStream::connect(endpoint),
            )
            .await
            .map_err(|_| ControllerError::new("timeout", "timed out connecting to FluidNC Telnet"))?
            .map_err(|error| ControllerError::new("connect", error.to_string()))?;
            stream
                .set_nodelay(true)
                .map_err(|error| ControllerError::new("connect", error.to_string()))?;
            session.socket = Some(FluidNcConnection::Telnet(stream));
            session.rx_buffer.clear();
            session.telnet_parse_state = TelnetParseState::Data;
            let id = format!("fluidnc:Telnet:{endpoint}");
            if let Ok(mut connected) = self.connected_device_id.write() {
                *connected = Some(id.clone());
            }
            if let Ok(mut selected) = self.selected_device_id.write() {
                *selected = Some(id);
            }
            self.auto_connected
                .store(true, std::sync::atomic::Ordering::SeqCst);
            tracing::info!(%endpoint, "connected to FluidNC over Telnet");
            let _ = self.events.send(CncEvent::Connected);
            return Ok(());
        }

        if self.discovery_enabled
            && self
                .auto_connected
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(ControllerError::new(
                "disconnected",
                "FluidNC connection was lost; reconnect manually",
            ));
        }

        if !self.discovery_enabled {
            let url = self.url.clone().ok_or_else(|| {
                ControllerError::new("connect", "FluidNC WebSocket URL is not configured")
            })?;
            let (socket, _) =
                tokio::time::timeout(self.config.connect_timeout, connect_async(&url))
                    .await
                    .map_err(|_| {
                        ControllerError::new("timeout", "timed out connecting to FluidNC")
                    })?
                    .map_err(|error| ControllerError::new("connect", error.to_string()))?;
            session.socket = Some(FluidNcConnection::WebSocket(socket));
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
            // The cached fast path is strictly WebSocket-only. A Telnet endpoint
            // must never be silently redirected to TCP port 80.
            let Some(url) = cached_websocket_url(endpoint) else {
                continue;
            };
            let address = endpoint
                .rsplit_once(':')
                .and_then(|(host, _)| host.parse::<std::net::Ipv4Addr>().ok())
                .expect("cached WebSocket URL validated its IPv4 host");
            match tokio::time::timeout(self.config.connect_timeout, connect_async(&url)).await {
                Ok(Ok((socket, _))) => {
                    session.socket = Some(FluidNcConnection::WebSocket(socket));
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
            // Automatic discovery prefers Telnet when both transports identify
            // the same controller. The machine is then controlled on the exact
            // port that the Telnet probe verified, not silently rerouted to 80.
            if device.transport == DiscoveryTransport::Telnet {
                let endpoint = device.endpoint.clone();
                tracing::info!(%endpoint, "connecting to first confirmed FluidNC Telnet endpoint");
                match tokio::time::timeout(
                    self.config.connect_timeout,
                    tokio::net::TcpStream::connect(&endpoint),
                )
                .await
                {
                    Ok(Ok(stream)) => {
                        let _ = stream.set_nodelay(true);
                        session.socket = Some(FluidNcConnection::Telnet(stream));
                        session.telnet_parse_state = TelnetParseState::Data;
                        self.auto_connected
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        if let Ok(mut connected) = self.connected_device_id.write() {
                            *connected = Some(device.id.clone());
                        }
                        if let Ok(mut selected) = self.selected_device_id.write() {
                            *selected = Some(device.id.clone());
                        }
                        drop(discoveries);
                        tracing::info!(%endpoint, "connected to first discovered FluidNC over Telnet");
                        let _ = self.events.send(CncEvent::Connected);
                        session.rx_buffer.clear();
                        return Ok(());
                    }
                    Ok(Err(error)) => {
                        tracing::warn!(%endpoint, %error, "failed to connect to discovered FluidNC Telnet; trying next match");
                    }
                    Err(_) => {
                        tracing::warn!(%endpoint, "timed out connecting to discovered FluidNC Telnet; trying next match");
                    }
                }
                continue;
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
                    session.socket = Some(FluidNcConnection::WebSocket(socket));
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
            "no usable FluidNC Telnet or WebSocket endpoint found",
        ))
    }
}
