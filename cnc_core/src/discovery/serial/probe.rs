//! Blocking per-port probing for GRBL-family and FluidNC controllers.

use std::{
    io::{Read, Write},
    time::Instant,
};

use tokio_serial::{ClearBuffer, SerialPort};

use super::identity::machine_name_of;
use super::identity::{
    extract_fluidnc_ipv4, fluidnc_banner_version, fluidnc_serial_device, grbl_mega_serial_device,
    grbl_serial_device, is_supported_grbl_version, parse_banner_version,
};
use super::{SERIAL_PROBE_DEADLINE, SERIAL_PROBE_IO_TIMEOUT, SERIAL_PROBE_MAX_BUFFER};
use crate::discovery::{
    DiscoveredDevice, DiscoveryTransport, FluidNcIdentity, DEFAULT_FLUIDNC_WEBSOCKET_PORT,
};

/// Blocking single-port GRBL/FluidNC probe (runs on `spawn_blocking`).
/// Returns every discovery record a confirmed controller produces: GRBL and
/// unrecognized ports yield none; a serial FluidNC yields its controllable
/// `fluidnc:serial:<port>` row plus — when the banner exposes a Wi-Fi address
/// — the `fluidnc:WebSocket:…` row for the same machine (the machines dialog
/// groups both variants under one header).
pub(super) fn probe_grbl_port_sync(port_path: String, baud: u32) -> Vec<DiscoveredDevice> {
    // The probe holds the port only for the deadline window; a port owned by
    // a live adapter session fails the open (or resets the board) and is
    // pre-excluded by the caller via the serial device ids.
    // `timeout` bounds the blocking read; POSIX serial reads then return
    // `Ok(0)` (or EAGAIN-mapped error) after the idle gap instead of hanging.
    let mut port = match tokio_serial::new(&port_path, baud)
        .timeout(SERIAL_PROBE_IO_TIMEOUT)
        .open_native()
    {
        Ok(port) => port,
        // Make inaccessible candidate ports visible in logs. In particular,
        // Linux commonly exposes `/dev/ttyACM*` as `root:dialout` with mode 0660;
        // the scanner cannot identify FluidNC until the backend user can open it.
        Err(error) => {
            tracing::warn!(port = %port_path, %error, "cannot open serial discovery candidate");
            return Vec::new();
        }
    };
    // Drop whatever was buffered before we connected; it is not an answer
    // to our ping.
    let _ = port.clear(ClearBuffer::Input);

    let deadline = Instant::now() + SERIAL_PROBE_DEADLINE;
    let mut collected: Vec<u8> = Vec::new();
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 256];
    let mut pings = 0_u32;
    let mut status_seen = false;
    // A GRBL-shaped boot banner is tentative until `$I` identity has a chance
    // to answer: some FluidNC builds print that banner before `[VER:FluidNC]`.
    let mut grbl_version: Option<String> = None;
    let mut unsupported_grbl_banner = false;
    // FluidNC state captured while streaming (task: COM-port FluidNC
    // detection). Any `[VER:FluidNC…]`/`[Boot:FluidNC…]` line marks the
    // controller; the version is best-effort.
    let mut fluidnc_seen = false;
    let mut fluidnc_version: Option<String> = None;

    while Instant::now() < deadline {
        // The realtime `?` byte needs no line terminator and is answered
        // with one status report; a board that missed its boot banner on
        // open still reveals itself here. The first ping additionally
        // requests the `$I` identity, which FluidNC answers with the
        // `[VER:FluidNC …]` banner GRBL rejects with a plain error.
        let request = if pings == 0 {
            b"?\n$I\n".as_slice()
        } else {
            b"?".as_slice()
        };
        if port.write_all(request).and_then(|_| port.flush()).is_err() {
            return Vec::new();
        }
        pings += 1;

        match port.read(&mut chunk) {
            Ok(0) => {} // idle gap within the probe window: keep waiting
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                collected.extend_from_slice(&chunk[..n]);
            }
            Err(_) if Instant::now() >= deadline => break,
            Err(_) => {}
        }
        if buffer.len() > SERIAL_PROBE_MAX_BUFFER {
            // Chatty non-controller device; stop collecting and bail out.
            return Vec::new();
        }
        if collected.len() > SERIAL_PROBE_MAX_BUFFER {
            // Trim the banner memory to the newest half; old text was already
            // scanned on the previous passes.
            let keep = SERIAL_PROBE_MAX_BUFFER / 2;
            let cut = collected.len() - keep;
            collected.drain(..cut);
        }

        // Evaluate every complete line received so far.
        while let Some(line_end) = buffer.iter().position(|byte| *byte == b'\n') {
            let raw: Vec<u8> = buffer.drain(..=line_end).collect();
            let line = String::from_utf8_lossy(&raw[..raw.len() - 1])
                .trim()
                .to_string();

            if let Some(version) = parse_banner_version(&line) {
                if is_supported_grbl_version(Some(&version)) {
                    grbl_version = Some(version);
                } else {
                    unsupported_grbl_banner = true;
                    tracing::warn!(
                        port = %port_path,
                        version = %version,
                        "serial boot banner is older than supported GRBL; continuing to check for FluidNC identity"
                    );
                }
            }
            // FluidNC identity/boot banner: unmistakable, checked before the
            // ambiguous status line because FluidNC shares the GRBL status
            // grammar (`<Idle|MPos:...>`).
            if !fluidnc_seen {
                if let Some(version) = fluidnc_banner_version(&line) {
                    fluidnc_seen = true;
                    fluidnc_version = Some(version);
                } else if line.starts_with("[Boot:FluidNC") {
                    fluidnc_seen = true;
                }
            }
            // Once the identity appeared, publish the controllable serial row
            // (task: FluidNC must work over the COM port). When the banner
            // also exposes a Wi-Fi address, add the network variant of the
            // same machine so both interfaces appear grouped in the dialog.
            if fluidnc_seen {
                let identity_text = String::from_utf8_lossy(&collected).into_owned();
                let machine_name = machine_name_of(&identity_text);
                let mut devices = vec![fluidnc_serial_device(
                    &port_path,
                    fluidnc_version.clone(),
                    machine_name.clone(),
                )];
                if let Some(address) = extract_fluidnc_ipv4(&identity_text) {
                    devices.push(DiscoveredDevice::fluidnc(
                        address,
                        DEFAULT_FLUIDNC_WEBSOCKET_PORT,
                        DiscoveryTransport::WebSocket,
                        FluidNcIdentity {
                            version: fluidnc_version.clone(),
                            machine_name,
                        },
                    ));
                }
                return devices;
            }
            // A pipe-separated `<...|...>` report is the 1.x status grammar.
            if line.starts_with('<') && line.ends_with('>') && line.contains('|') {
                status_seen = true;
            }
        }

        // Do not early-return on the GRBL-shaped banner or status report. Wait
        // through the bounded probe window so FluidNC's `$I` identity response
        // can override the tentative GRBL classification.
    }

    if grbl_version.is_some() || (status_seen && !unsupported_grbl_banner) {
        tracing::debug!(port = %port_path, ?grbl_version, "serial controller identified as GRBL");
        let is_mega = grbl_version.as_deref().is_some_and(|version| {
            let version = version.to_ascii_lowercase();
            version.contains("mega") || version.starts_with("1.1m")
        });
        let device = if is_mega {
            grbl_mega_serial_device(&port_path, grbl_version)
        } else {
            grbl_serial_device(&port_path, grbl_version)
        };
        return vec![device];
    }
    tracing::debug!(port = %port_path, "serial discovery probe found no supported controller identity");
    Vec::new()
}
