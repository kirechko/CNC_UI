//! WebSocket frame transport for [`FluidNcWebSocketAdapter`]: text/binary
//! writes, line extraction from the receive buffer and the single-command
//! acknowledgement loop. The `CncApi` trait implementation lives in `api.rs`.

use super::{
    FluidNcConnection, FluidNcWebSocketAdapter, Session, TelnetParseState, MAX_RESPONSE_LINE,
};
use crate::{CncResult, ControllerError};
use futures_util::{SinkExt, StreamExt};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

impl FluidNcWebSocketAdapter {
    /// Optional wire-trace hook; compiled out to a no-op unless the vendor
    /// trace build replaces the body.
    pub(super) fn trace_frame(_direction: &str, _opcode: &str, _payload: &[u8]) {}

    /// Emit a microsecond-stamped program event when `CNC_UI_FLUIDNC_WS_TRACE` is set.
    pub(super) fn trace_program_event(event: &str, line: u32, _detail: &str) {
        if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
            static TRACE_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
            let elapsed_us = TRACE_START.get_or_init(Instant::now).elapsed().as_micros();
            eprintln!("[FluidNC] +{elapsed_us}us line={line} {event}");
        }
    }

    /// Send one text frame, appending the newline FluidNC expects, and drop the
    /// session if the write itself fails.
    pub(super) async fn send_text(&self, session: &mut Session, text: &str) -> CncResult<()> {
        // FluidNC terminates a G-code command with a newline. For example, the WebUI
        // frame "$G" (3 bytes) contains "$G\n".
        let payload = format!("{}\n", text.trim_end_matches(['\r', '\n']));
        Self::trace_frame("send", "text", payload.as_bytes());
        tracing::debug!(command = %text, "sending FluidNC line");
        let result = match session.socket.as_mut() {
            Some(FluidNcConnection::WebSocket(socket)) => {
                timeout(self.config.ack_timeout, socket.send(Message::Text(payload)))
                    .await
                    .map_err(|_| {
                        ControllerError::new("timeout", "timed out sending command to FluidNC")
                    })?
                    .map_err(|error| ControllerError::new("io", error.to_string()))
            }
            Some(FluidNcConnection::Telnet(stream)) => timeout(
                self.config.ack_timeout,
                stream.write_all(payload.as_bytes()),
            )
            .await
            .map_err(|_| ControllerError::new("timeout", "timed out sending Telnet command"))?
            .map_err(|error| ControllerError::new("io", error.to_string())),
            None => Err(ControllerError::new(
                "connection",
                "FluidNC is disconnected",
            )),
        };

        if result.is_err() {
            self.invalidate_connection(session);
        }
        result
    }

    /// Send one realtime binary byte (status request, feed hold, jog cancel...).
    pub(super) async fn send_realtime(&self, session: &mut Session, byte: u8) -> CncResult<()> {
        if byte != b'?' {
            Self::trace_frame("send", "binary", &[byte]);
        }
        let result = match session.socket.as_mut() {
            Some(FluidNcConnection::WebSocket(socket)) => timeout(
                self.config.ack_timeout,
                socket.send(Message::Binary(vec![byte])),
            )
            .await
            .map_err(|_| {
                ControllerError::new("timeout", "timed out sending realtime command to FluidNC")
            })?
            .map_err(|error| ControllerError::new("io", error.to_string())),
            Some(FluidNcConnection::Telnet(stream)) => {
                timeout(self.config.ack_timeout, stream.write_all(&[byte]))
                    .await
                    .map_err(|_| {
                        ControllerError::new("timeout", "timed out sending Telnet realtime command")
                    })?
                    .map_err(|error| ControllerError::new("io", error.to_string()))
            }
            None => Err(ControllerError::new(
                "connection",
                "FluidNC is disconnected",
            )),
        };

        if result.is_err() {
            self.invalidate_connection(session);
        }
        result
    }

    /// Pop one newline-terminated line (without `\r\n`) from the receive buffer.
    pub(super) fn extract_line(buffer: &mut Vec<u8>) -> CncResult<Option<String>> {
        let Some(end) = buffer.iter().position(|byte| *byte == b'\n') else {
            if buffer.len() > MAX_RESPONSE_LINE {
                return Err(ControllerError::new(
                    "protocol",
                    "FluidNC response line exceeds size limit",
                ));
            }
            return Ok(None);
        };

        if end > MAX_RESPONSE_LINE {
            return Err(ControllerError::new(
                "protocol",
                "FluidNC response line exceeds size limit",
            ));
        }

        let mut line: Vec<u8> = buffer.drain(..=end).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Ok(Some(String::from_utf8_lossy(&line).trim().to_owned()))
    }

    /// Read the next complete controller line, waiting at most `duration` per
    /// frame and skipping the WebUI `currentID:` bookkeeping frames.
    pub(super) async fn read_line(session: &mut Session, duration: Duration) -> CncResult<String> {
        loop {
            if let Some(line) = Self::extract_line(&mut session.rx_buffer)? {
                return Ok(line);
            }

            let mut chunk = [0_u8; 1024];
            match session.socket.as_mut() {
                Some(FluidNcConnection::WebSocket(socket)) => {
                    let message = timeout(duration, socket.next())
                        .await
                        .map_err(|_| {
                            ControllerError::new(
                                "timeout",
                                "timed out waiting for FluidNC response",
                            )
                        })?
                        .ok_or_else(|| {
                            ControllerError::new("disconnected", "FluidNC WebSocket closed")
                        })?
                        .map_err(|error| ControllerError::new("io", error.to_string()))?;
                    match message {
                        Message::Text(text) => {
                            Self::trace_frame("recv", "text", text.as_bytes());
                            if !text.starts_with("currentID:") && !text.starts_with("CURRENT_ID:") {
                                session.rx_buffer.extend_from_slice(text.as_bytes());
                            }
                        }
                        Message::Binary(bytes) => session.rx_buffer.extend_from_slice(&bytes),
                        Message::Ping(_) | Message::Pong(_) => {}
                        Message::Close(_) => {
                            session.socket = None;
                            return Err(ControllerError::new(
                                "disconnected",
                                "FluidNC WebSocket closed",
                            ));
                        }
                        _ => {}
                    }
                }
                Some(FluidNcConnection::Telnet(stream)) => {
                    let count = timeout(duration, stream.read(&mut chunk))
                        .await
                        .map_err(|_| {
                            ControllerError::new(
                                "timeout",
                                "timed out waiting for FluidNC Telnet response",
                            )
                        })?
                        .map_err(|error| ControllerError::new("io", error.to_string()))?;
                    if count == 0 {
                        session.socket = None;
                        return Err(ControllerError::new(
                            "disconnected",
                            "FluidNC Telnet connection closed",
                        ));
                    }
                    let mut payload = Vec::with_capacity(count);
                    let mut replies = Vec::new();
                    Self::parse_telnet_bytes(
                        &chunk[..count],
                        &mut session.telnet_parse_state,
                        &mut payload,
                        &mut replies,
                    );
                    if !replies.is_empty() {
                        stream
                            .write_all(&replies)
                            .await
                            .map_err(|error| ControllerError::new("io", error.to_string()))?;
                    }
                    session.rx_buffer.extend_from_slice(&payload);
                }
                None => {
                    return Err(ControllerError::new(
                        "connection",
                        "FluidNC is disconnected",
                    ))
                }
            }
        }
    }

    /// Remove Telnet negotiation bytes while replying to unsupported options with WONT/DONT.
    fn parse_telnet_bytes(
        bytes: &[u8],
        state: &mut TelnetParseState,
        payload: &mut Vec<u8>,
        replies: &mut Vec<u8>,
    ) {
        const IAC: u8 = 255;
        const DONT: u8 = 254;
        const DO: u8 = 253;
        const WONT: u8 = 252;
        const WILL: u8 = 251;
        const SB: u8 = 250;
        const SE: u8 = 240;
        for byte in bytes.iter().copied() {
            match *state {
                TelnetParseState::Data if byte == IAC => *state = TelnetParseState::Iac,
                TelnetParseState::Data => payload.push(byte),
                TelnetParseState::Iac => match byte {
                    IAC => {
                        payload.push(IAC);
                        *state = TelnetParseState::Data;
                    }
                    WILL | WONT | DO | DONT => *state = TelnetParseState::Negotiation(byte),
                    SB => *state = TelnetParseState::Subnegotiation,
                    _ => *state = TelnetParseState::Data,
                },
                TelnetParseState::Negotiation(verb) => {
                    match verb {
                        WILL => replies.extend_from_slice(&[IAC, DONT, byte]),
                        DO => replies.extend_from_slice(&[IAC, WONT, byte]),
                        _ => {}
                    }
                    *state = TelnetParseState::Data;
                }
                TelnetParseState::Subnegotiation if byte == IAC => {
                    *state = TelnetParseState::SubnegotiationIac
                }
                TelnetParseState::Subnegotiation => {}
                TelnetParseState::SubnegotiationIac if byte == SE => {
                    *state = TelnetParseState::Data
                }
                TelnetParseState::SubnegotiationIac if byte == IAC => {
                    *state = TelnetParseState::Subnegotiation
                }
                TelnetParseState::SubnegotiationIac => *state = TelnetParseState::Subnegotiation,
            }
        }
    }

    /// Write one G-code/settings command and wait for its `ok`, tolerating
    /// interleaved status reports and banner lines.
    pub(super) async fn send_command(&self, session: &mut Session, command: &str) -> CncResult<()> {
        if command.contains(['\n', '\r']) {
            return Err(ControllerError::new(
                "bad_command",
                "a FluidNC command must contain exactly one line",
            ));
        }
        if command.trim().is_empty() {
            return Ok(());
        }

        self.send_text(session, command.trim()).await?;
        let result = match timeout(self.config.ack_timeout, async {
            loop {
                let line = Self::read_line(session, self.config.ack_timeout).await?;
                if line == "ok" {
                    if let Some(system) = Self::coordinate_system_select(command) {
                        session.active_coordinate_system = system;
                    }
                    return Ok(());
                }
                if let Some(error) = line.strip_prefix("error:") {
                    tracing::warn!(controller_error = %error, "FluidNC rejected command");
                    return Err(ControllerError::new(
                        "controller",
                        format!("FluidNC rejected command ({error})"),
                    ));
                }
                if line.starts_with("ALARM:") {
                    tracing::error!(alarm = %line, "FluidNC alarm");
                    return Err(ControllerError::new("alarm", line));
                }
                // FluidNC may interleave status reports, startup banners and [GC:...] messages.
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC command acknowledgement",
            )),
        };

        // Only transport-level failures (timeouts, I/O, closed socket) drop the
        // session. A controller `error:N` or `ALARM:` reply rejects this one command
        // while the socket stays healthy, so the session is kept and the error is
        // returned to the caller.
        if result.as_ref().is_err_and(|error| {
            !matches!(error.code.as_str(), "gcode_error" | "alarm" | "controller")
        }) {
            self.invalidate_connection(session);
        }
        result
    }
}
