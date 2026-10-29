//! Unit tests for the discovery module: CIDR bounds and target ordering
//! (against `targets`), FluidNC `$I` identity parsing and Telnet negotiation
//! filtering (against `probe`) and per-transport device identity.

use super::{
    fluidnc_websocket_address,
    probe::{
        fluidnc_identity, parse_telnet_bytes, TelnetParseState, TELNET_DO, TELNET_DONT, TELNET_IAC,
        TELNET_WILL, TELNET_WONT,
    },
    targets::{discovery_targets, Ipv4Cidr},
    DiscoveredDevice, DiscoveryTransport, FluidNcIdentity,
};
use std::net::Ipv4Addr;

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
        fluidnc_identity("[VER:4.1 FluidNC v4.1.1 (build 123)]\n[MSG:Machine:Test Router]\nok\n"),
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
