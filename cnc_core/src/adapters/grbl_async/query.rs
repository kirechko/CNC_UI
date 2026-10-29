//! Realtime status (`?`) and settings (`$$`) queries shared by the trait
//! facade and the streaming scheduler.

use super::{GrblAdapter, Session};
use crate::{CncResult, ControllerError};
use std::{collections::HashMap, time::Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

impl GrblAdapter {
    /// Consumes the boot chatter GRBL emits when the serial port is opened.
    ///
    /// Opening the port on Linux asserts DTR/RTS, which resets most Arduino
    /// boards: they print a banner (`Grbl 1.1h ['$' for help]`) plus assorted
    /// `[MSG:...]`/`ALARM:` lines. Leftovers of that chatter would later be
    /// misread as replies to real commands, so the handshake pings `?` until
    /// the controller proves itself (banner line or `<...>` status report)
    /// and then waits for the byte flow to go quiet, discarding every boot
    /// message. It is best-effort by design: a board that never answers
    /// (port busy, controller off) simply ends the handshake at the overall
    /// deadline, and the regular error handling of the first real query then
    /// reports the actual problem instead of poisoning the session here.
    pub(super) async fn run_startup_handshake(session: &mut Session) {
        session.handshake_pending = false;
        session.rx_buffer.clear();

        let deadline = Instant::now() + super::HANDSHAKE_TIMEOUT;
        let mut last_ping = Instant::now();
        let mut last_activity = Instant::now();
        let mut confirmed = false;
        let mut chunk = [0_u8; 256];

        while Instant::now() < deadline {
            // Ping while the controller has not proven itself yet. The
            // realtime `?` byte needs no ACK, so a failed write is not fatal
            // here: later status queries surface real I/O errors themselves.
            if !confirmed && Instant::now() - last_ping >= super::HANDSHAKE_PING_INTERVAL {
                let _ = session.port.write_all(b"?").await;
                last_ping = Instant::now();
            }

            // Once confirmed only a short gap has to stay silent, so read in
            // small windows and leave as soon as the flow stops.
            let read_window = if confirmed {
                super::HANDSHAKE_DRAIN_GAP
            } else {
                super::HANDSHAKE_PING_INTERVAL
            };
            let read_window = read_window.min(deadline.saturating_duration_since(Instant::now()));

            match timeout(read_window, session.port.read(&mut chunk)).await {
                // EOF or I/O failure: give up silently, the real query reports it.
                Ok(Ok(0)) | Ok(Err(_)) => return,
                Ok(Ok(n)) => {
                    last_activity = Instant::now();
                    session.rx_buffer.extend_from_slice(&chunk[..n]);
                    // Parse and discard every whole line accumulated so far.
                    loop {
                        match Self::extract_line(&mut session.rx_buffer) {
                            Ok(Some(line)) => {
                                if (line.starts_with('<') && line.ends_with('>'))
                                    || line.starts_with("Grbl ")
                                {
                                    confirmed = true;
                                }
                            }
                            Ok(None) => break,
                            Err(_) => {
                                // Oversized or non-UTF-8 boot noise: drop it
                                // so it can never leak into a later command.
                                session.rx_buffer.clear();
                                break;
                            }
                        }
                    }
                }
                Err(_) => {
                    // No bytes during this window.
                    if confirmed && last_activity.elapsed() >= super::HANDSHAKE_DRAIN_GAP {
                        return;
                    }
                }
            }
        }
    }

    /// Sends the realtime `?` request and returns the `<...>` status report
    /// line. Unlike `send_command`, a timeout here does not poison the
    /// protocol: a lost status reply does not desynchronize the ACK queue.
    /// The first call after [`GrblAdapter::connect`] runs the startup
    /// handshake because opening the port resets Arduino boards.
    pub(super) async fn query_status(session: &mut Session) -> CncResult<String> {
        if session.handshake_pending {
            Self::run_startup_handshake(session).await;
        }
        Self::ensure_protocol_usable(session.protocol_poisoned)?;
        if let Err(error) = session.port.write_all(b"?").await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }

        match timeout(super::STATUS_TIMEOUT, async {
            loop {
                let line = match Self::read_line(
                    &mut session.port,
                    &mut session.rx_buffer,
                    super::STATUS_TIMEOUT,
                )
                .await
                {
                    Ok(line) => line,
                    Err(error) => {
                        session.protocol_poisoned = true;
                        return Err(error);
                    }
                };
                if line.starts_with('<') && line.ends_with('>') {
                    return Ok(line);
                }
            }
        })
        .await
        {
            Ok(result) => result,
            // A lost response to the realtime `?` request does not desynchronize the command ACK queue.
            // Do not block further requests and commands because of a single timeout.
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for GRBL status",
            )),
        }
    }

    /// Runs `$$` and collects every `$N=value` setting until the final `ok`.
    /// A timeout poisons the protocol because the settings reply is a normal
    /// command transaction.
    pub(super) async fn query_settings(session: &mut Session) -> CncResult<HashMap<u16, f64>> {
        // Same startup synchronization as the command path: `$$` is often the
        // very first exchange (worker boot reads the machine configuration),
        // and the pending boot chatter must not break the reply collection.
        if session.handshake_pending {
            Self::run_startup_handshake(session).await;
        }
        Self::ensure_protocol_usable(session.protocol_poisoned)?;
        if let Err(error) = session.port.write_all(b"$$\n").await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }

        match timeout(super::RESPONSE_TIMEOUT, async {
            let mut settings = HashMap::new();

            loop {
                let line = match Self::read_line(
                    &mut session.port,
                    &mut session.rx_buffer,
                    super::RESPONSE_TIMEOUT,
                )
                .await
                {
                    Ok(line) => line,
                    Err(error) => {
                        session.protocol_poisoned = true;
                        return Err(error);
                    }
                };
                if line == "ok" {
                    return Ok(settings);
                }
                if let Some(error) = line.strip_prefix("error:") {
                    return Err(ControllerError::new("settings", error));
                }
                if line.starts_with("ALARM:") {
                    return Err(ControllerError::new("alarm", line));
                }

                let Some(setting) = line.strip_prefix('$') else {
                    continue;
                };
                let Some((number, value)) = setting.split_once('=') else {
                    continue;
                };
                let (Ok(number), Ok(value)) = (number.parse::<u16>(), value.parse::<f64>()) else {
                    continue;
                };
                if value.is_finite() {
                    settings.insert(number, value);
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => {
                session.protocol_poisoned = true;
                Err(ControllerError::new(
                    "timeout",
                    "timed out waiting for GRBL settings; reconnect required",
                ))
            }
        }
    }
}
