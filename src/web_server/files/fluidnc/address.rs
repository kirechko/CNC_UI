//! FluidNC machine identity to HTTP address resolution.

use super::super::super::state::AppState;
use super::super::FsError;

/// Derives `host:port` of the controller HTTP server from a discovered
/// machine id (`fluidnc:<Transport>:<endpoint>`). Serial and socket transports
/// have no FluidNC HTTP server and yield `None`.
fn parse_machine_id(id: &str) -> Option<String> {
    let tail = id.strip_prefix("fluidnc:")?;
    let (transport, endpoint) = tail.split_once(':')?;
    match transport {
        "WebSocket" => {
            // Endpoint already carries the discovered port (80 by default).
            Some(if endpoint.contains(':') {
                endpoint.to_string()
            } else {
                format!("{endpoint}:80")
            })
        }
        "Telnet" => {
            let host = endpoint.rsplit(':').next_back()?;
            Some(format!("{host}:80"))
        }
        _ => None,
    }
}

/// HTTP address of the currently connected FluidNC controller.
///
/// Network transports resolve their address from the machine id. A controller
/// connected over a COM port resolves through its Wi-Fi twin — the
/// `fluidnc:WebSocket:…` row the serial scanner registered under the same
/// machine name — so the FILES panel keeps working while jog/program traffic
/// stays on the cable (task: HTTP file exchange for serial FluidNC). Without a
/// twin (Wi-Fi disabled) there is genuinely no HTTP server to talk to and the
/// dedicated error keeps the controller/sd source chips unavailable.
pub(crate) fn device_address(state: &AppState) -> Result<String, FsError> {
    let id = state
        .connected_machine_id()
        .ok_or_else(|| FsError::new("files.error.notConnected", ""))?;
    if !id.starts_with("fluidnc:") {
        return Err(FsError::new("files.error.notFluidNc", ""));
    }
    if let Some(address) = parse_machine_id(&id) {
        return Ok(address);
    }
    // Serial connection: match the network twin by machine name (the banner
    // `[MSG:Machine:…]` value both discovery rows carry).
    let snapshot = state.machine_snapshot();
    let name = snapshot
        .iter()
        .find(|device| device.id == id)
        .and_then(|device| device.machine_name.clone());
    if let Some(name) = name {
        let twin = snapshot.iter().find(|device| {
            device.protocol == "FluidNC"
                && device.transport == cnc_core::discovery::DiscoveryTransport::WebSocket
                && device.machine_name.as_deref() == Some(name.as_str())
        });
        if let Some(address) = twin.and_then(|device| parse_machine_id(&device.id)) {
            return Ok(address);
        }
    }
    Err(FsError::new("files.error.serialNoHttp", id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_parsing_selects_http_addresses() {
        assert_eq!(
            parse_machine_id("fluidnc:WebSocket:192.168.1.50:80").as_deref(),
            Some("192.168.1.50:80")
        );
        // Port-less endpoints still get the default web port.
        assert_eq!(
            parse_machine_id("fluidnc:WebSocket:10.0.0.5").as_deref(),
            Some("10.0.0.5:80")
        );
        // Telnet discovery found the serial port; HTTP still lives on 80.
        assert_eq!(
            parse_machine_id("fluidnc:Telnet:192.168.1.50:23").as_deref(),
            Some("192.168.1.50:80")
        );
        assert_eq!(parse_machine_id("fluidnc:Serial:/dev/ttyUSB0"), None);
        assert_eq!(parse_machine_id("emulator:tcp_socket:127.0.0.1:8765"), None);
    }
}
