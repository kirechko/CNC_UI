//! Target-set computation for discovery: which IPv4 addresses are worth
//! probing. Combines explicit configuration entries, local interface
//! networks, a Docker/default-route gateway /24 and router hops found by a
//! UDP-traceroute heuristic, then orders cached (priority) endpoints first.
//! The CIDR helper type and the route helpers are shared with the scan
//! orchestration in the sibling `scan` module via `pub(super)` visibility.

use super::DISCOVERY_MAX_PREFIX;
use crate::{CncResult, ControllerError};
use std::{collections::HashSet, net::Ipv4Addr};

/// Maximum router hops walked by the UDP-traceroute heuristic before giving up.
const DISCOVERY_MAX_TRACE_HOPS: u8 = 8;

/// Per-hop receive timeout while listening for ICMP "time exceeded" errors.
const DISCOVERY_TRACE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);

/// Builds the ordered probe target list: priority endpoints first (deduped,
/// even when outside every known network), then every remaining address of
/// the union of interface networks, explicit CIDRs, traced router networks and
/// the private default-route gateway /24, sorted for deterministic scans.
pub(super) fn discovery_targets(
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

/// True for RFC1918 and link-local ranges: the only hops the traceroute
/// heuristic is allowed to expand into /24 scans (public routers are skipped).
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

/// RAII guard closing the raw libc socket descriptor used by the traceroute.
#[cfg(target_os = "linux")]
struct SocketFd(libc::c_int);

#[cfg(target_os = "linux")]
impl Drop for SocketFd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// Reads the default-route gateway from `/proc/net/route` (Linux only), used
/// to cover Docker networks where the UI browser and controller often live.
pub(super) fn default_route_gateway() -> Option<Ipv4Addr> {
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

/// Runs the blocking UDP traceroute on a worker thread and converts every
/// private router hop into a bounded /24 network for the route-derived scan.
pub(super) async fn traced_private_networks() -> Vec<Ipv4Cidr> {
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

/// Converts a netmask address into its CIDR prefix length.
fn prefix_len(netmask: Ipv4Addr) -> u8 {
    u32::from(netmask).leading_ones() as u8
}

/// IPv4 network with a CIDR prefix, bounded so a single entry cannot expand
/// into an unbounded scan of the whole Internet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Ipv4Cidr {
    network: u32,
    prefix: u8,
}

impl Ipv4Cidr {
    /// Parses `a.b.c.d/nn`, rejecting prefixes wider than the discovery bound.
    pub(super) fn parse(value: &str) -> Result<Self, String> {
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

    /// Builds the network containing `address` with the given prefix, clamped
    /// to the discovery bound so wide interface masks never explode the scan.
    pub(super) fn from_interface(address: Ipv4Addr, prefix: u8) -> Self {
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

    /// Iterates usable host addresses, skipping the network and broadcast
    /// addresses for prefixes narrower than /31.
    pub(super) fn addresses(self) -> impl Iterator<Item = Ipv4Addr> {
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
