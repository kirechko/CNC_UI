//! FluidNC identity probes: connects to one (address, port, transport)
//! candidate, sends the `$I` status command and parses the banner until a
//! complete `ok`-terminated identity response arrives (or the probe fails /
//! exceeds its byte budget). Also hosts the incremental Telnet negotiation
//! parser so the probe stream stays free of option bytes.

use super::FluidNcIdentity;
use futures_util::{SinkExt, StreamExt};
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// Per-operation timeout for every connect / send / receive step of a probe.
const DISCOVERY_PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);

/// Abort a probe once its accumulated `$I` response exceeds this many bytes;
/// legitimate FluidNC banners are far shorter, oversized streams mean noise.
const DISCOVERY_IDENTITY_MAX_BYTES: usize = 4096;

/// Probes one host over WebSocket and extracts the FluidNC identity from `$I`.
pub(super) async fn probe_fluidnc_websocket(
    address: Ipv4Addr,
    port: u16,
) -> Option<FluidNcIdentity> {
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

/// Probes one host over raw Telnet and extracts the FluidNC identity from `$I`.
pub(super) async fn probe_fluidnc_telnet(address: Ipv4Addr, port: u16) -> Option<FluidNcIdentity> {
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

// Telnet protocol control bytes used by the incremental negotiation parser.
// `pub(super)` lets the unit tests (sibling module) build negotiation sequences
// byte by byte without duplicating the protocol constants.
pub(super) const TELNET_IAC: u8 = 255;
pub(super) const TELNET_DONT: u8 = 254;
pub(super) const TELNET_DO: u8 = 253;
pub(super) const TELNET_WONT: u8 = 252;
pub(super) const TELNET_WILL: u8 = 251;
const TELNET_SB: u8 = 250;
const TELNET_SE: u8 = 240;

/// State machine position while decoding Telnet option negotiation that can
/// span arbitrary read boundaries. `pub(super)` so the unit tests can drive
/// the parser directly across artificial read boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TelnetParseState {
    Data,
    Iac,
    Negotiation(u8),
    Subnegotiation,
    SubnegotiationIac,
}

/// Filters Telnet negotiation out of `bytes`, appending plain payload to
/// `payload` and refusal replies (WONT/DONT) to `replies`.
pub(super) fn parse_telnet_bytes(
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

/// Extracts firmware version and machine name from a complete `$I` response;
/// returns `None` until an `ok` acknowledgement line closes the response.
pub(super) fn fluidnc_identity(response: &str) -> Option<FluidNcIdentity> {
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
