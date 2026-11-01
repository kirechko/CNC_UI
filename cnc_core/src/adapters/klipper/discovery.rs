//! mDNS discovery of Moonraker services with cached endpoint priority support.

use crate::discovery::{DiscoveredDevice, DiscoveryTransport};
use crate::{CncResult, ControllerError};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::time::Duration;

/// Runs bounded mDNS discovery off the async executor thread.
async fn browse_mdns() -> CncResult<Vec<DiscoveredDevice>> {
    tokio::task::spawn_blocking(|| {
        let daemon =
            ServiceDaemon::new().map_err(|e| ControllerError::new("discovery", e.to_string()))?;
        let receiver = daemon
            .browse("_moonraker._tcp.local.")
            .map_err(|e| ControllerError::new("discovery", e.to_string()))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        let mut devices = Vec::new();
        while std::time::Instant::now() < deadline {
            let Ok(ServiceEvent::ServiceResolved(info)) =
                receiver.recv_timeout(Duration::from_millis(250))
            else {
                continue;
            };
            let address = info
                .get_addresses()
                .iter()
                .next()
                .map(ToString::to_string)
                .unwrap_or_else(|| info.get_hostname().trim_end_matches('.').to_owned());
            let name = info
                .get_fullname()
                .trim_end_matches("._moonraker._tcp.local.")
                .to_owned();
            push_unique(
                &mut devices,
                DiscoveredDevice {
                    id: format!("klipper:{name}"),
                    protocol: "Klipper".into(),
                    machine_name: Some(name),
                    version: info.get_property_val_str("version").map(str::to_owned),
                    transport: DiscoveryTransport::WebSocket,
                    endpoint: format!("http://{address}:{}", info.get_port()),
                },
            );
        }
        let _ = daemon.stop_browse("_moonraker._tcp.local.");
        let _ = daemon.shutdown();
        Ok(devices)
    })
    .await
    .map_err(|e| ControllerError::new("discovery", e.to_string()))?
}

/// Browses `_moonraker._tcp.local.` and returns confirmed service endpoints.
pub async fn discover(priority_targets: Vec<String>) -> CncResult<Vec<DiscoveredDevice>> {
    let mut devices = Vec::new();
    for endpoint in priority_targets {
        if let Some(device) = probe_cached(&endpoint).await {
            push_unique(&mut devices, device);
        }
    }
    for device in browse_mdns().await? {
        push_unique(&mut devices, device);
    }
    Ok(devices)
}

/// Accepts a cached Moonraker URL only when it responds to `server.info`.
async fn probe_cached(endpoint: &str) -> Option<DiscoveredDevice> {
    let address = endpoint.strip_prefix("http://")?.split('/').next()?;
    let mut stream = tokio::time::timeout(
        Duration::from_millis(800),
        tokio::net::TcpStream::connect(address),
    )
    .await
    .ok()?
    .ok()?;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let path = "/server/info";
    let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut response = Vec::new();
    tokio::time::timeout(
        Duration::from_millis(800),
        stream.read_to_end(&mut response),
    )
    .await
    .ok()?
    .ok()?;
    let text = String::from_utf8_lossy(&response);
    let body = text.split_once("\r\n\r\n")?.1;
    let info: serde_json::Value = serde_json::from_str(body).ok()?;
    let host = address.split(':').next().unwrap_or(address);
    let machine = info
        .get("result")?
        .get("hostname")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(host);
    Some(DiscoveredDevice {
        id: format!("klipper:{machine}"),
        protocol: "Klipper".into(),
        machine_name: Some(machine.to_owned()),
        version: info
            .get("result")?
            .get("moonraker_version")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        transport: DiscoveryTransport::WebSocket,
        endpoint: endpoint.trim_end_matches('/').to_owned(),
    })
}

/// Keeps the first occurrence of each stable endpoint identity.
fn push_unique(devices: &mut Vec<DiscoveredDevice>, device: DiscoveredDevice) {
    if !devices
        .iter()
        .any(|entry| entry.id == device.id || entry.endpoint == device.endpoint)
    {
        devices.push(device);
    }
}
