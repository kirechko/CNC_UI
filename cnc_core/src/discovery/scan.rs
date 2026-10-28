//! Scan orchestration for FluidNC discovery: turns the configured network
//! targets into concrete (address, port, transport) probe candidates, runs the
//! bounded-concurrency probe pool and streams confirmed devices back to the
//! caller. The public entry points defined here are re-exported from the
//! parent `discovery` module so external call paths stay unchanged.

use super::{
    DISCOVERY_MAX_PREFIX, DiscoveredDevice, DiscoveryTransport, FluidNcDiscoveryConfig,
    probe::{probe_fluidnc_telnet, probe_fluidnc_websocket},
    targets::{Ipv4Cidr, default_route_gateway, discovery_targets, traced_private_networks},
};
use crate::{CncResult, ControllerError};
use std::{
    collections::HashSet,
    net::Ipv4Addr,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::{sync::mpsc, task::JoinSet};

/// Upper bound on simultaneously in-flight network probes; also the capacity
/// of the streaming result channel so a slow consumer cannot deadlock the pool.
const DISCOVERY_MAX_CONCURRENT_PROBES: usize = 64;

/// Number of scans currently running process-wide, reported in tracing so
/// overlapping UI refreshes and adapter reconnects are visible in the logs.
static ACTIVE_DISCOVERY_SCANS: AtomicUsize = AtomicUsize::new(0);

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
