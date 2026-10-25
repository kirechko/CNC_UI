//! Shared CNC device discovery primitives and FluidNC network probes.
//!
//! The result and transport types are deliberately broader than the FluidNC
//! network scanners so serial and local-socket discovery can be added here
//! without adapter-specific result formats.

use crate::{CncResult, ControllerError};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    net::Ipv4Addr,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
    task::JoinSet,
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const DISCOVERY_MAX_PREFIX: u8 = 24;
const DISCOVERY_MAX_CONCURRENT_PROBES: usize = 64;
const DISCOVERY_PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);
const DISCOVERY_MAX_TRACE_HOPS: u8 = 8;
const DISCOVERY_TRACE_TIMEOUT: Duration = Duration::from_millis(250);
const DISCOVERY_IDENTITY_MAX_BYTES: usize = 4096;
const DEFAULT_FLUIDNC_WEBSOCKET_PORT: u16 = 80;
const DEFAULT_FLUIDNC_TELNET_PORT: u16 = 23;
static ACTIVE_DISCOVERY_SCANS: AtomicUsize = AtomicUsize::new(0);

/// Connection mechanism for a discovered CNC controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryTransport {
    WebSocket,
    Telnet,
    Serial,
    UnixSocket,
    TcpSocket,
}

impl DiscoveryTransport {
    pub fn label(self) -> &'static str {
        match self {
            Self::WebSocket => "WebSocket",
            Self::Telnet => "Telnet",
            Self::Serial => "COM-порт",
            Self::UnixSocket => "Unix-сокет",
            Self::TcpSocket => "TCP-сокет",
        }
    }
}

/// A controller endpoint found by any discovery backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    /// Stable identifier suitable for comparing results across API layers.
    pub id: String,
    /// Firmware/controller family, for example `FluidNC` or `LinuxCNC`.
    pub protocol: String,
    /// Machine name reported by the controller, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
    /// Firmware version, when reported by the device identity response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub transport: DiscoveryTransport,
    /// Human-readable endpoint; e.g. `192.168.1.20:80`, `/dev/ttyUSB0`, or a socket path.
    pub endpoint: String,
}

impl DiscoveredDevice {
    fn fluidnc(
        address: Ipv4Addr,
        port: u16,
        transport: DiscoveryTransport,
        identity: FluidNcIdentity,
    ) -> Self {
        let endpoint = format!("{address}:{port}");
        Self {
            id: format!("fluidnc:{transport:?}:{endpoint}"),
            protocol: "FluidNC".into(),
            machine_name: identity.machine_name,
            version: identity.version,
            transport,
            endpoint,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FluidNcIdentity {
    version: Option<String>,
    machine_name: Option<String>,
}

/// Network settings for the automatic FluidNC discovery scan.
#[derive(Debug, Clone)]
pub struct FluidNcDiscoveryConfig {
    /// IPv4 hosts or CIDRs to include. CIDRs wider than `/24` are rejected.
    pub explicit_targets: Vec<String>,
    /// Previously discovered endpoints to probe before the general scan.
    pub priority_targets: Vec<String>,
    /// Hosts that must not be probed on any transport, such as a machine that is
    /// already under an active control session (a new probe can disturb it).
    pub excluded_hosts: Vec<Ipv4Addr>,
    pub websocket_port: u16,
    pub telnet_port: u16,
}

impl Default for FluidNcDiscoveryConfig {
    fn default() -> Self {
        Self {
            explicit_targets: Vec::new(),
            priority_targets: Vec::new(),
            excluded_hosts: Vec::new(),
            websocket_port: DEFAULT_FLUIDNC_WEBSOCKET_PORT,
            telnet_port: DEFAULT_FLUIDNC_TELNET_PORT,
        }
    }
}

/// Start scanning cached endpoints first, then immediately available networks,
/// followed by route-derived networks. Confirmed endpoints are streamed as probes finish.
pub async fn discover_fluidnc_stream(
    config: &FluidNcDiscoveryConfig,
) -> CncResult<mpsc::Receiver<DiscoveredDevice>> {
    let docker_gateway = default_route_gateway();
    let priority_candidates = priority_candidates(
        &config.priority_targets,
        config.websocket_port,
        config.telnet_port,
    );
    let immediate_targets = discovery_targets(&config.explicit_targets, &[], &[], None)?;
    let gateway_targets: Vec<Ipv4Addr> = docker_gateway
        .map(|gateway| {
            Ipv4Cidr::from_interface(gateway, DISCOVERY_MAX_PREFIX)
                .addresses()
                .collect()
        })
        .unwrap_or_default();
    if immediate_targets.is_empty() && gateway_targets.is_empty() && priority_candidates.is_empty()
    {
        return Err(ControllerError::new(
            "discovery",
            "no IPv4 targets available",
        ));
    }

    let websocket_port = config.websocket_port;
    let telnet_port = config.telnet_port;
    let excluded_hosts = config.excluded_hosts.clone();
    let active_scans = ACTIVE_DISCOVERY_SCANS.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::info!(
        active_scans,
        excluded_hosts = ?excluded_hosts,
        priority_targets = config.priority_targets.len(),
        explicit_targets = config.explicit_targets.len(),
        "FluidNC discovery scan started"
    );
    if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
        eprintln!(
            "[FluidNC discovery] started active_scans={active_scans} excluded_hosts={excluded_hosts:?} priority_targets={} explicit_targets={}",
            config.priority_targets.len(),
            config.explicit_targets.len(),
        );
    }
    let (sender, receiver) = mpsc::channel(DISCOVERY_MAX_CONCURRENT_PROBES);
    tokio::spawn(async move {
        if let Some(gateway) = docker_gateway {
            tracing::info!(%gateway, "including the default-route gateway /24 in FluidNC discovery");
        }
        tracing::info!("starting FluidNC WebSocket and Telnet discovery");

        let route_scan = tokio::spawn(traced_private_networks());
        let priority_addresses: HashSet<_> = priority_candidates
            .iter()
            .map(|(address, _, _)| *address)
            .collect();
        scan_candidates(
            without_excluded_hosts(priority_candidates, &excluded_hosts),
            &sender,
        )
        .await;

        let immediate_set: HashSet<_> = immediate_targets.iter().copied().collect();
        let gateway_set: HashSet<_> = gateway_targets.iter().copied().collect();
        let local_candidates = candidates_for_targets(
            immediate_targets
                .into_iter()
                .filter(|address| !priority_addresses.contains(address)),
            websocket_port,
            telnet_port,
        );
        let gateway_candidates = candidates_for_targets(
            gateway_targets.into_iter().filter(|address| {
                !priority_addresses.contains(address) && !immediate_set.contains(address)
            }),
            websocket_port,
            telnet_port,
        );
        let local_scan = scan_candidates(
            without_excluded_hosts(local_candidates, &excluded_hosts),
            &sender,
        );
        let gateway_scan = scan_candidates(
            without_excluded_hosts(gateway_candidates, &excluded_hosts),
            &sender,
        );
        let route_sender = sender.clone();
        let route_scan = async move {
            let traced_networks = match route_scan.await {
                Ok(networks) => networks,
                Err(error) => {
                    tracing::debug!(%error, "route discovery task failed");
                    Vec::new()
                }
            };
            let traced_targets = discovery_targets(&[], &[], &traced_networks, None)
                .unwrap_or_default()
                .into_iter()
                .filter(|address| {
                    !priority_addresses.contains(address)
                        && !immediate_set.contains(address)
                        && !gateway_set.contains(address)
                });
            scan_candidates(
                without_excluded_hosts(
                    candidates_for_targets(traced_targets, websocket_port, telnet_port),
                    &excluded_hosts,
                ),
                &route_sender,
            )
            .await;
        };
        tokio::join!(local_scan, gateway_scan, route_scan);
        let active_scans = ACTIVE_DISCOVERY_SCANS.fetch_sub(1, Ordering::SeqCst) - 1;
        tracing::info!(active_scans, "FluidNC discovery scan finished");
        if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
            eprintln!("[FluidNC discovery] finished active_scans={active_scans}");
        }
    });
    Ok(receiver)
}

fn priority_candidates(
    endpoints: &[String],
    websocket_port: u16,
    telnet_port: u16,
) -> Vec<(Ipv4Addr, u16, DiscoveryTransport)> {
    // A cached endpoint identifies a host, not a transport. Probe both the
    // WebSocket and Telnet ports of every cached host; otherwise a host cached
    // via Telnet is never checked on the WebSocket port and disappears from the
    // WebSocket results (and vice versa).
    let mut seen = HashSet::new();
    let addresses: Vec<Ipv4Addr> = endpoints
        .iter()
        .filter_map(|endpoint| {
            let host = endpoint
                .rsplit_once(':')
                .map_or(endpoint.as_str(), |(host, _)| host);
            host.parse::<Ipv4Addr>().ok()
        })
        .filter(|address| seen.insert(*address))
        .collect();
    candidates_for_targets(addresses, websocket_port, telnet_port)
}

fn candidates_for_targets(
    targets: impl IntoIterator<Item = Ipv4Addr>,
    websocket_port: u16,
    telnet_port: u16,
) -> Vec<(Ipv4Addr, u16, DiscoveryTransport)> {
    targets
        .into_iter()
        .flat_map(|address| {
            [
                (address, websocket_port, DiscoveryTransport::WebSocket),
                (address, telnet_port, DiscoveryTransport::Telnet),
            ]
        })
        .collect()
}

/// Drops every probe (WebSocket and Telnet) for excluded hosts.
fn without_excluded_hosts(
    candidates: Vec<(Ipv4Addr, u16, DiscoveryTransport)>,
    excluded: &[Ipv4Addr],
) -> Vec<(Ipv4Addr, u16, DiscoveryTransport)> {
    candidates
        .into_iter()
        .filter(|(address, _, _)| !excluded.contains(address))
        .collect()
}

async fn scan_candidates(
    candidates: Vec<(Ipv4Addr, u16, DiscoveryTransport)>,
    sender: &mpsc::Sender<DiscoveredDevice>,
) {
    let mut candidates = candidates.into_iter();
    let mut probes = JoinSet::new();
    let mut in_flight = 0usize;
    while in_flight < DISCOVERY_MAX_CONCURRENT_PROBES {
        let Some((address, port, transport)) = candidates.next() else {
            break;
        };
        probes.spawn(async move {
            let identity = match transport {
                DiscoveryTransport::WebSocket => probe_fluidnc_websocket(address, port).await,
                DiscoveryTransport::Telnet => probe_fluidnc_telnet(address, port).await,
                _ => None,
            };
            (address, port, transport, identity)
        });
        in_flight += 1;
    }

    while in_flight > 0 {
        if let Some(Ok((address, port, transport, identity))) = probes.join_next().await {
            in_flight -= 1;
            if let Some(identity) = identity {
                tracing::info!(%address, port, ?transport, ?identity, "FluidNC discovered");
                if sender
                    .send(DiscoveredDevice::fluidnc(
                        address, port, transport, identity,
                    ))
                    .await
                    .is_err()
                {
                    // The receiver is gone (for example, the session connected to a
                    // machine). Stop starting probes; dropping JoinSet aborts the rest.
                    tracing::info!(%address, port, ?transport, "FluidNC found after discovery receiver closed; stopping scan");
                    return;
                }
            }
            if let Some((address, port, transport)) = candidates.next() {
                probes.spawn(async move {
                    let identity = match transport {
                        DiscoveryTransport::WebSocket => {
                            probe_fluidnc_websocket(address, port).await
                        }
                        DiscoveryTransport::Telnet => probe_fluidnc_telnet(address, port).await,
                        _ => None,
                    };
                    (address, port, transport, identity)
                });
                in_flight += 1;
            }
        } else {
            break;
        }
    }
}

/// Collect a complete scan while preserving successful probe completion order.
pub async fn discover_fluidnc(config: &FluidNcDiscoveryConfig) -> CncResult<Vec<DiscoveredDevice>> {
    let mut receiver = discover_fluidnc_stream(config).await?;
    let mut discovered = Vec::new();
    while let Some(device) = receiver.recv().await {
        discovered.push(device);
    }
    Ok(discovered)
}

/// Returns the currently discovered WebSocket candidate for existing FluidNC
/// adapter connection selection.
pub fn fluidnc_websocket_address(device: &DiscoveredDevice) -> Option<Ipv4Addr> {
    if device.protocol != "FluidNC" || device.transport != DiscoveryTransport::WebSocket {
        return None;
    }
    let host = device.endpoint.rsplit_once(':')?.0;
    host.parse().ok()
}

fn discovery_targets(
    discovery_entries: &[String],
    priority_entries: &[String],
    heuristic_networks: &[Ipv4Cidr],
    docker_gateway: Option<Ipv4Addr>,
) -> CncResult<Vec<Ipv4Addr>> {
    let mut networks = HashSet::new();
    for interface in if_addrs::get_if_addrs().map_err(|error| {
        ControllerError::new(
            "discovery",
            format!("failed to enumerate interfaces: {error}"),
        )
    })? {
        if let if_addrs::IfAddr::V4(address) = interface.addr {
            if !address.ip.is_loopback() {
                networks.insert(Ipv4Cidr::from_interface(
                    address.ip,
                    prefix_len(address.netmask),
                ));
            }
        }
    }

    for entry in discovery_entries {
        if entry.contains('/') {
            networks.insert(
                Ipv4Cidr::parse(entry)
                    .map_err(|message| ControllerError::new("configuration", message))?,
            );
        } else {
            let address: Ipv4Addr = entry.parse().map_err(|_| {
                ControllerError::new(
                    "configuration",
                    format!("discovery target must be an IPv4 host or CIDR: {entry}"),
                )
            })?;
            networks.insert(Ipv4Cidr::from_interface(address, 32));
        }
    }

    networks.extend(heuristic_networks.iter().copied());
    if let Some(gateway) = docker_gateway.filter(|address| is_private_ipv4(*address)) {
        networks.insert(Ipv4Cidr::from_interface(gateway, DISCOVERY_MAX_PREFIX));
    }

    let mut targets = HashSet::new();
    for network in networks {
        for address in network.addresses() {
            if !address.is_broadcast() && !address.is_unspecified() && !address.is_multicast() {
                targets.insert(address);
            }
        }
    }

    let mut ordered = Vec::with_capacity(targets.len() + priority_entries.len());
    let mut prioritized = HashSet::new();
    for endpoint in priority_entries {
        let host = endpoint
            .rsplit_once(':')
            .map_or(endpoint.as_str(), |(host, _)| host);
        if let Ok(address) = host.parse::<Ipv4Addr>() {
            if prioritized.insert(address) {
                targets.remove(&address);
                ordered.push(address);
            }
        }
    }
    let mut remaining: Vec<_> = targets.into_iter().collect();
    remaining.sort_unstable();
    ordered.extend(remaining);
    Ok(ordered)
}

fn is_private_ipv4(address: Ipv4Addr) -> bool {
    address.is_private() || address.is_link_local()
}

#[cfg(target_os = "linux")]
fn trace_private_hops_to(destination: Ipv4Addr) -> Vec<Ipv4Addr> {
    use std::mem::{size_of, zeroed};

    let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if socket < 0 {
        tracing::debug!(error = %std::io::Error::last_os_error(), "could not create UDP socket for route discovery");
        return Vec::new();
    }
    let _socket_guard = SocketFd(socket);
    let enabled: libc::c_int = 1;
    let enabled_result = unsafe {
        libc::setsockopt(
            socket,
            libc::IPPROTO_IP,
            libc::IP_RECVERR,
            &enabled as *const _ as *const libc::c_void,
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if enabled_result != 0 {
        tracing::debug!(error = %std::io::Error::last_os_error(), "could not enable ICMP error queue for route discovery");
        return Vec::new();
    }

    let mut hops = HashSet::new();
    for ttl in 1..=DISCOVERY_MAX_TRACE_HOPS {
        let ttl_value = ttl as libc::c_int;
        if unsafe {
            libc::setsockopt(
                socket,
                libc::IPPROTO_IP,
                libc::IP_TTL,
                &ttl_value as *const _ as *const libc::c_void,
                size_of::<libc::c_int>() as libc::socklen_t,
            )
        } != 0
        {
            break;
        }
        let mut target: libc::sockaddr_in = unsafe { zeroed() };
        target.sin_family = libc::AF_INET as libc::sa_family_t;
        target.sin_port = (33434u16 + ttl as u16).to_be();
        target.sin_addr = libc::in_addr {
            s_addr: u32::from(destination).to_be(),
        };
        let payload = b"cnc-device-discovery";
        if unsafe {
            libc::sendto(
                socket,
                payload.as_ptr() as *const libc::c_void,
                payload.len(),
                0,
                &target as *const _ as *const libc::sockaddr,
                size_of::<libc::sockaddr_in>() as libc::socklen_t,
            )
        } < 0
        {
            continue;
        }

        let mut poll_fd = libc::pollfd {
            fd: socket,
            events: libc::POLLERR,
            revents: 0,
        };
        if unsafe {
            libc::poll(
                &mut poll_fd,
                1,
                DISCOVERY_TRACE_TIMEOUT.as_millis() as libc::c_int,
            )
        } <= 0
        {
            continue;
        }
        let mut payload_buffer = [0u8; 1];
        let mut control = [0usize; 64];
        let mut message: libc::msghdr = unsafe { zeroed() };
        let mut iovec = libc::iovec {
            iov_base: payload_buffer.as_mut_ptr() as *mut libc::c_void,
            iov_len: payload_buffer.len(),
        };
        message.msg_iov = &mut iovec;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr() as *mut libc::c_void;
        message.msg_controllen = size_of::<[usize; 64]>();
        if unsafe {
            libc::recvmsg(
                socket,
                &mut message,
                libc::MSG_ERRQUEUE | libc::MSG_DONTWAIT,
            )
        } < 0
        {
            continue;
        }
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                if (*header).cmsg_level == libc::IPPROTO_IP
                    && (*header).cmsg_type == libc::IP_RECVERR
                    && (*header).cmsg_len
                        >= libc::CMSG_LEN(size_of::<libc::sock_extended_err>() as u32) as usize
                {
                    let extended = libc::CMSG_DATA(header) as *const libc::sock_extended_err;
                    if (*extended).ee_origin == libc::SO_EE_ORIGIN_ICMP
                        && ((*extended).ee_type == 11 || (*extended).ee_type == 3)
                    {
                        let offender = libc::SO_EE_OFFENDER(extended) as *const libc::sockaddr_in;
                        if !offender.is_null()
                            && (*offender).sin_family as libc::c_int == libc::AF_INET
                        {
                            let address = Ipv4Addr::from(u32::from_be((*offender).sin_addr.s_addr));
                            if is_private_ipv4(address) {
                                hops.insert(address);
                            }
                            if (*extended).ee_type == 3 {
                                break;
                            }
                        }
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
    }
    let mut hops: Vec<_> = hops.into_iter().collect();
    hops.sort_unstable();
    hops
}

#[cfg(target_os = "linux")]
fn trace_private_hops() -> Vec<Ipv4Addr> {
    let mut hops = HashSet::new();
    for destination in [
        Ipv4Addr::new(1, 1, 1, 1),
        Ipv4Addr::new(8, 8, 8, 8),
        Ipv4Addr::new(9, 9, 9, 9),
    ] {
        hops.extend(trace_private_hops_to(destination));
    }
    let mut hops: Vec<_> = hops.into_iter().collect();
    hops.sort_unstable();
    hops
}

#[cfg(not(target_os = "linux"))]
fn trace_private_hops() -> Vec<Ipv4Addr> {
    Vec::new()
}

#[cfg(target_os = "linux")]
struct SocketFd(libc::c_int);

#[cfg(target_os = "linux")]
impl Drop for SocketFd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

fn default_route_gateway() -> Option<Ipv4Addr> {
    #[cfg(target_os = "linux")]
    {
        let routes = std::fs::read_to_string("/proc/net/route").ok()?;
        for line in routes.lines().skip(1) {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 4 || fields[1] != "00000000" {
                continue;
            }
            let flags = u16::from_str_radix(fields[3], 16).ok()?;
            if flags & 0x2 == 0 {
                continue;
            }
            let raw_gateway = u32::from_str_radix(fields[2], 16).ok()?;
            let gateway = Ipv4Addr::from(raw_gateway.to_le());
            if !gateway.is_unspecified() {
                return Some(gateway);
            }
        }
    }
    None
}

async fn traced_private_networks() -> Vec<Ipv4Cidr> {
    match tokio::task::spawn_blocking(trace_private_hops).await {
        Ok(hops) => {
            for hop in &hops {
                tracing::info!(%hop, "private router hop discovered by UDP traceroute heuristic");
            }
            hops.into_iter()
                .map(|address| Ipv4Cidr::from_interface(address, DISCOVERY_MAX_PREFIX))
                .collect()
        }
        Err(error) => {
            tracing::debug!(%error, "route discovery task failed");
            Vec::new()
        }
    }
}

fn prefix_len(netmask: Ipv4Addr) -> u8 {
    u32::from(netmask).leading_ones() as u8
}

async fn probe_fluidnc_websocket(address: Ipv4Addr, port: u16) -> Option<FluidNcIdentity> {
    let url = format!("ws://{address}:{port}/");
    let Ok(Ok((mut socket, _))) = timeout(DISCOVERY_PROBE_TIMEOUT, connect_async(url)).await else {
        return None;
    };
    if timeout(
        DISCOVERY_PROBE_TIMEOUT,
        socket.send(Message::Text("$I\n".into())),
    )
    .await
    .map_or(true, |result| result.is_err())
    {
        return None;
    }
    let mut response = String::new();
    loop {
        let Ok(Some(Ok(message))) = timeout(DISCOVERY_PROBE_TIMEOUT, socket.next()).await else {
            return None;
        };
        match message {
            Message::Text(text) => {
                if !response.is_empty() {
                    response.push('\n');
                }
                response.push_str(&text);
            }
            Message::Binary(bytes) => {
                if !response.is_empty() {
                    response.push('\n');
                }
                response.push_str(&String::from_utf8_lossy(&bytes));
            }
            Message::Ping(payload) => {
                if timeout(DISCOVERY_PROBE_TIMEOUT, socket.send(Message::Pong(payload)))
                    .await
                    .map_or(true, |r| r.is_err())
                {
                    return None;
                }
            }
            Message::Close(_) => return None,
            _ => continue,
        }
        if response.len() > DISCOVERY_IDENTITY_MAX_BYTES {
            return None;
        }
        if let Some(version) = fluidnc_identity(&response) {
            return Some(version);
        }
    }
}

async fn probe_fluidnc_telnet(address: Ipv4Addr, port: u16) -> Option<FluidNcIdentity> {
    let target = (address, port);
    let Ok(Ok(mut stream)) = timeout(DISCOVERY_PROBE_TIMEOUT, TcpStream::connect(target)).await
    else {
        return None;
    };
    if timeout(DISCOVERY_PROBE_TIMEOUT, stream.write_all(b"$I\n"))
        .await
        .map_or(true, |r| r.is_err())
    {
        return None;
    }
    let mut response = Vec::with_capacity(256);
    let mut chunk = [0u8; 512];
    let mut telnet_state = TelnetParseState::Data;
    loop {
        let Ok(Ok(read)) = timeout(DISCOVERY_PROBE_TIMEOUT, stream.read(&mut chunk)).await else {
            return None;
        };
        if read == 0 {
            return None;
        }
        let mut payload = Vec::with_capacity(read);
        let mut replies = Vec::new();
        parse_telnet_bytes(
            &chunk[..read],
            &mut telnet_state,
            &mut payload,
            &mut replies,
        );
        if !replies.is_empty()
            && timeout(DISCOVERY_PROBE_TIMEOUT, stream.write_all(&replies))
                .await
                .map_or(true, |result| result.is_err())
        {
            return None;
        }
        response.extend_from_slice(&payload);
        if response.len() > DISCOVERY_IDENTITY_MAX_BYTES {
            return None;
        }
        if let Some(version) = fluidnc_identity(&String::from_utf8_lossy(&response)) {
            return Some(version);
        }
    }
}

const TELNET_IAC: u8 = 255;
const TELNET_DONT: u8 = 254;
const TELNET_DO: u8 = 253;
const TELNET_WONT: u8 = 252;
const TELNET_WILL: u8 = 251;
const TELNET_SB: u8 = 250;
const TELNET_SE: u8 = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TelnetParseState {
    Data,
    Iac,
    Negotiation(u8),
    Subnegotiation,
    SubnegotiationIac,
}

fn parse_telnet_bytes(
    bytes: &[u8],
    state: &mut TelnetParseState,
    payload: &mut Vec<u8>,
    replies: &mut Vec<u8>,
) {
    for byte in bytes.iter().copied() {
        match *state {
            TelnetParseState::Data if byte == TELNET_IAC => *state = TelnetParseState::Iac,
            TelnetParseState::Data => payload.push(byte),
            TelnetParseState::Iac => match byte {
                TELNET_IAC => {
                    payload.push(TELNET_IAC);
                    *state = TelnetParseState::Data;
                }
                TELNET_WILL | TELNET_WONT | TELNET_DO | TELNET_DONT => {
                    *state = TelnetParseState::Negotiation(byte);
                }
                TELNET_SB => *state = TelnetParseState::Subnegotiation,
                _ => *state = TelnetParseState::Data,
            },
            TelnetParseState::Negotiation(verb) => {
                match verb {
                    TELNET_WILL => replies.extend_from_slice(&[TELNET_IAC, TELNET_DONT, byte]),
                    TELNET_DO => replies.extend_from_slice(&[TELNET_IAC, TELNET_WONT, byte]),
                    _ => {}
                }
                *state = TelnetParseState::Data;
            }
            TelnetParseState::Subnegotiation if byte == TELNET_IAC => {
                *state = TelnetParseState::SubnegotiationIac;
            }
            TelnetParseState::Subnegotiation => {}
            TelnetParseState::SubnegotiationIac if byte == TELNET_SE => {
                *state = TelnetParseState::Data;
            }
            TelnetParseState::SubnegotiationIac if byte == TELNET_IAC => {
                *state = TelnetParseState::Subnegotiation;
            }
            TelnetParseState::SubnegotiationIac => {
                *state = TelnetParseState::Subnegotiation;
            }
        }
    }
}

fn fluidnc_identity(response: &str) -> Option<FluidNcIdentity> {
    let has_ack = response.lines().any(|line| line.trim() == "ok");
    if !has_ack {
        return None;
    }

    let banner = response.lines().find_map(|line| {
        let line = line.trim();
        line.starts_with("[VER:")
            .then(|| line.strip_prefix("[VER:")?.strip_suffix(']'))
            .flatten()
            .filter(|banner| banner.contains("FluidNC"))
    })?;
    let version = banner.split_once("FluidNC").and_then(|(_, firmware)| {
        let firmware = firmware.trim();
        let version = firmware
            .strip_prefix('v')
            .or_else(|| firmware.strip_prefix("version "))?
            .split_whitespace()
            .next()?
            .trim_matches(|character: char| !character.is_ascii_alphanumeric() && character != '.')
            .to_string();
        (!version.is_empty()).then_some(version)
    });
    let machine_name = response.lines().find_map(|line| {
        let value = line
            .trim()
            .strip_prefix("[MSG:Machine:")?
            .strip_suffix(']')?;
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    });
    Some(FluidNcIdentity {
        version,
        machine_name,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Ipv4Cidr {
    network: u32,
    prefix: u8,
}

impl Ipv4Cidr {
    fn parse(value: &str) -> Result<Self, String> {
        let (address, prefix) = value
            .split_once('/')
            .ok_or_else(|| format!("CIDR must include a prefix: {value}"))?;
        let address: Ipv4Addr = address
            .parse()
            .map_err(|_| format!("invalid IPv4 address in CIDR: {value}"))?;
        let prefix: u8 = prefix
            .parse()
            .map_err(|_| format!("invalid IPv4 prefix in CIDR: {value}"))?;
        if prefix < DISCOVERY_MAX_PREFIX {
            return Err(format!(
                "discovery CIDR {value} is broader than /{DISCOVERY_MAX_PREFIX}"
            ));
        }
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        Ok(Self {
            network: u32::from(address) & mask,
            prefix,
        })
    }

    fn from_interface(address: Ipv4Addr, prefix: u8) -> Self {
        let prefix = prefix.max(DISCOVERY_MAX_PREFIX);
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        Self {
            network: u32::from(address) & mask,
            prefix,
        }
    }

    fn addresses(self) -> impl Iterator<Item = Ipv4Addr> {
        let host_bits = 32 - self.prefix;
        let count = 1u64 << host_bits;
        let first = self.network as u64;
        let start = if self.prefix < 31 { 1 } else { 0 };
        let end = if self.prefix < 31 {
            count.saturating_sub(1)
        } else {
            count
        };
        (start..end).filter_map(move |offset| {
            let value = first + offset;
            (value <= u32::MAX as u64).then(|| Ipv4Addr::from(value as u32))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_cidr_is_bounded_to_slash_24_or_narrower() {
        assert!(Ipv4Cidr::parse("192.168.4.0/24").is_ok());
        assert!(Ipv4Cidr::parse("192.168.4.0/25").is_ok());
        assert!(Ipv4Cidr::parse("192.168.0.0/16").is_err());
        assert!(Ipv4Cidr::parse("192.168.4.0").is_err());
    }

    #[test]
    fn explicit_cidr_and_single_host_are_included() {
        let targets = discovery_targets(&["10.20.30.0/24".into()], &[], &[], None).unwrap();
        assert!(targets.contains(&Ipv4Addr::new(10, 20, 30, 1)));
        assert!(targets.contains(&Ipv4Addr::new(10, 20, 30, 254)));
        let host = Ipv4Addr::new(10, 20, 30, 7);
        let targets = discovery_targets(&[host.to_string()], &[], &[], None).unwrap();
        assert!(targets.contains(&host));
        assert!(!targets.contains(&Ipv4Addr::new(10, 20, 30, 8)));
    }

    #[test]
    fn docker_gateway_network_is_bounded() {
        let targets = discovery_targets(&[], &[], &[], Some(Ipv4Addr::new(172, 17, 0, 1))).unwrap();
        assert!(targets.contains(&Ipv4Addr::new(172, 17, 0, 254)));
        assert!(!targets.contains(&Ipv4Addr::new(172, 17, 1, 1)));
    }

    #[test]
    fn cached_endpoints_are_probed_first_even_outside_current_networks() {
        let targets = discovery_targets(
            &["10.0.0.0/24".into()],
            &["192.168.50.162:80".into()],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(targets[0], Ipv4Addr::new(192, 168, 50, 162));
        assert!(targets.contains(&Ipv4Addr::new(10, 0, 0, 1)));
    }

    #[test]
    fn identity_extracts_fluidnc_metadata_and_requires_complete_ack() {
        assert_eq!(
            fluidnc_identity("[VER:4.1 FluidNC v4.1.1]\n[MSG:Machine:My CNC]\nok\n"),
            Some(FluidNcIdentity {
                version: Some("4.1.1".into()),
                machine_name: Some("My CNC".into()),
            })
        );
        assert_eq!(fluidnc_identity("[VER:1.1 GRBL]\nok\n"), None);
        assert_eq!(
            fluidnc_identity("[VER:4.1 FluidNC v4.1.1]\n[MSG:Machine:My CNC]\nnot-ok\n"),
            None
        );
        assert_eq!(
            fluidnc_identity("[VER:4.1 FluidNC]\nok\n"),
            Some(FluidNcIdentity {
                version: None,
                machine_name: None,
            })
        );
        assert_eq!(
            fluidnc_identity("[VER:4.1 FluidNC v4.1.1]\nok\n"),
            Some(FluidNcIdentity {
                version: Some("4.1.1".into()),
                machine_name: None,
            })
        );
        assert_eq!(
            fluidnc_identity(
                "[VER:4.1 FluidNC v4.1.1 (build 123)]\n[MSG:Machine:Test Router]\nok\n"
            ),
            Some(FluidNcIdentity {
                version: Some("4.1.1".into()),
                machine_name: Some("Test Router".into()),
            })
        );
    }

    #[test]
    fn telnet_negotiation_is_removed_and_unsupported_options_refused() {
        let mut state = TelnetParseState::Data;
        let mut payload = Vec::new();
        let mut replies = Vec::new();
        parse_telnet_bytes(
            &[
                b'[',
                TELNET_IAC,
                TELNET_WILL,
                1,
                b'V',
                TELNET_IAC,
                TELNET_DO,
                3,
                b'R',
            ],
            &mut state,
            &mut payload,
            &mut replies,
        );
        assert_eq!(payload, b"[VR");
        assert_eq!(
            replies,
            [TELNET_IAC, TELNET_DONT, 1, TELNET_IAC, TELNET_WONT, 3]
        );
    }

    #[test]
    fn telnet_negotiation_state_survives_read_boundaries() {
        let mut state = TelnetParseState::Data;
        let mut payload = Vec::new();
        let mut replies = Vec::new();
        parse_telnet_bytes(&[TELNET_IAC], &mut state, &mut payload, &mut replies);
        parse_telnet_bytes(
            &[TELNET_WILL, 1, b'o', b'k'],
            &mut state,
            &mut payload,
            &mut replies,
        );
        assert_eq!(payload, b"ok");
        assert_eq!(replies, [TELNET_IAC, TELNET_DONT, 1]);
    }

    #[test]
    fn device_identity_preserves_each_transport_variant() {
        let identity = FluidNcIdentity {
            version: Some("4.1.1".into()),
            machine_name: Some("My CNC".into()),
        };
        let ws = DiscoveredDevice::fluidnc(
            Ipv4Addr::new(192, 168, 1, 2),
            80,
            DiscoveryTransport::WebSocket,
            identity.clone(),
        );
        let telnet = DiscoveredDevice::fluidnc(
            Ipv4Addr::new(192, 168, 1, 2),
            23,
            DiscoveryTransport::Telnet,
            identity,
        );
        assert_ne!(ws.id, telnet.id);
        assert_eq!(
            fluidnc_websocket_address(&ws),
            Some(Ipv4Addr::new(192, 168, 1, 2))
        );
        assert_eq!(fluidnc_websocket_address(&telnet), None);
    }
}
