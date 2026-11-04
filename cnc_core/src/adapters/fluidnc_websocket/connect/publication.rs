//! Status event publication and cached discovery-table accessors.

use super::super::{device_id_host, FluidNcWebSocketAdapter};
use crate::discovery::DiscoveredDevice;
use crate::{machine_state, CncEvent, MachineState, MachineStatus};

impl FluidNcWebSocketAdapter {
    /// Push a status snapshot into the watch channel and translate meaningful
    /// diffs (state, program line, alarm entry) into broadcast events.
    pub(in crate::adapters::fluidnc_websocket) fn publish_status(&self, status: MachineStatus) {
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

    /// Cached discovery table for the UI. Only auto-selected WebSocket hosts are
    /// hidden; explicit Telnet/serial selections must remain discoverable so the
    /// operator can inspect or reselect their exact transport.
    pub fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        let selected_ws_host = self
            .selected_device_id
            .read()
            .ok()
            .and_then(|id| id.clone())
            .filter(|id| id.starts_with("fluidnc:WebSocket:"))
            .as_deref()
            .and_then(device_id_host);
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
                        selected_ws_host != device_host
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
}
