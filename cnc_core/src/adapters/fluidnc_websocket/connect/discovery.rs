//! Connection invalidation and manual rediscovery bookkeeping.

use super::super::{
    device_id_host, disconnected_status, FluidNcWebSocketAdapter, Session, FLUIDNC_PORT,
};
use crate::discovery::{discover_fluidnc_stream, DiscoveredDevice, FluidNcDiscoveryConfig};
use crate::{CncEvent, CncResult};
use tokio::sync::mpsc;

impl FluidNcWebSocketAdapter {
    /// Forget the socket and all transport state, publishing a disconnected
    /// snapshot when there was an active session to lose.
    #[track_caller]
    pub(in crate::adapters::fluidnc_websocket) fn invalidate_connection(
        &self,
        session: &mut Session,
    ) {
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
    pub(in crate::adapters::fluidnc_websocket) async fn start_rediscovery_stream(
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
