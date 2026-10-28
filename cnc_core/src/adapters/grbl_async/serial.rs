//! Line-level serial transport: command writing, acknowledgement reading and
//! the realtime-command escape hatch that never waits for an `ok`.

use super::{GrblAdapter, Session};
use crate::{ControllerError, CncResult};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};
use tokio_serial::SerialStream;

impl GrblAdapter {
    /// Hard-resets the protocol state after the UI cancelled a running
    /// command: drop pending streaming/jog, flush the RX buffer and send the
    /// soft-reset realtime byte (0x18) so the controller returns to a known
    /// state.
    pub(super) async fn reset_after_command_cancellation(session: &mut Session) -> CncResult<()> {
        session.protocol_poisoned = true;
        session.program_streaming = false;
        session.continuous_jog = None;
        session.jog_chunk_active = false;
        session.current_line = 0;
        session.rx_buffer.clear();
        session.spindle_command = crate::SpindleCommand {
            direction: crate::SpindleDirection::Stopped,
            rpm: 0.0,
        };
        Self::send_realtime(session, 0x18).await
    }

    /// Pops one complete `\n`-terminated line from the RX buffer, validating
    /// UTF-8 and the maximum line size. Returns `None` while no full line is
    /// buffered yet.
    pub(super) fn extract_line(rx_buffer: &mut Vec<u8>) -> CncResult<Option<String>> {
        if let Some(line_end) = rx_buffer.iter().position(|byte| *byte == b'\n') {
            if line_end > super::MAX_RESPONSE_LINE {
                return Err(ControllerError::new(
                    "protocol",
                    "GRBL response line exceeds size limit",
                ));
            }

            let line: Vec<u8> = rx_buffer.drain(..=line_end).collect();
            let line = std::str::from_utf8(&line[..line.len() - 1]).map_err(|error| {
                ControllerError::new(
                    "protocol",
                    format!("GRBL response is not valid UTF-8: {error}"),
                )
            })?;
            return Ok(Some(line.trim().to_string()));
        }

        if rx_buffer.len() > super::MAX_RESPONSE_LINE {
            return Err(ControllerError::new(
                "protocol",
                "GRBL response line exceeds size limit",
            ));
        }

        Ok(None)
    }

    /// Reads from the port until a whole line is available, then returns it.
    pub(super) async fn read_line(
        port: &mut SerialStream,
        rx_buffer: &mut Vec<u8>,
        read_timeout: Duration,
    ) -> CncResult<String> {
        loop {
            if let Some(line) = Self::extract_line(rx_buffer)? {
                return Ok(line);
            }

            let mut chunk = [0_u8; 256];
            let bytes_read = timeout(read_timeout, port.read(&mut chunk))
                .await
                .map_err(|_| {
                    ControllerError::new("timeout", "timed out waiting for GRBL response")
                })?
                .map_err(|error| ControllerError::new("io", error.to_string()))?;
            if bytes_read == 0 {
                return Err(ControllerError::new(
                    "disconnected",
                    "GRBL serial port closed",
                ));
            }
            rx_buffer.extend_from_slice(&chunk[..bytes_read]);
        }
    }

    /// A response timeout marks the command/ACK queue as desynchronized; every
    /// later operation must fail fast until the port is reopened.
    pub(super) fn ensure_protocol_usable(protocol_poisoned: bool) -> CncResult<()> {
        if protocol_poisoned {
            return Err(ControllerError::new(
                "protocol_desynchronized",
                "GRBL response timed out; reconnect before sending more commands",
            ));
        }
        Ok(())
    }

    /// Rejects anything GRBL cannot accept on a single line: newlines,
    /// non-printable bytes or overly long commands.
    pub(super) fn validate_command(command: &str) -> CncResult<()> {
        if command.contains(['\n', '\r']) {
            return Err(ControllerError::new(
                "bad_command",
                "a GRBL command must contain exactly one line",
            ));
        }
        if !command.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
            return Err(ControllerError::new(
                "bad_command",
                "GRBL commands must contain printable ASCII characters only",
            ));
        }

        let command = command.trim();
        if command.len() > super::MAX_COMMAND_LENGTH {
            return Err(ControllerError::new(
                "bad_command",
                format!(
                    "GRBL command exceeds the {}-byte line limit",
                    super::MAX_COMMAND_LENGTH
                ),
            ));
        }

        Ok(())
    }

    /// Writes one command line and waits for its `ok` acknowledgement.
    /// Startup banners and asynchronous status reports are skipped; an
    /// `error:`/`ALARM:` reply aborts immediately. A lost acknowledgement
    /// poisons the protocol so later calls fail fast.
    pub(super) async fn send_command(session: &mut Session, command: &str) -> CncResult<()> {
        Self::ensure_protocol_usable(session.protocol_poisoned)?;
        Self::validate_command(command)?;

        if command.trim().is_empty() {
            return Ok(());
        }

        if let Err(error) = session.port.write_all(command.trim().as_bytes()).await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }
        if let Err(error) = session.port.write_all(b"\n").await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }

        match timeout(super::RESPONSE_TIMEOUT, async {
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
                    if let Some(system) = Self::coordinate_system_select(command) {
                        session.active_coordinate_system = system;
                    }
                    return Ok(());
                }
                if let Some(error) = line.strip_prefix("error:") {
                    return Err(ControllerError::new(
                        "gcode_error",
                        format!("GRBL rejected command ({error}): {command}"),
                    ));
                }
                if line.starts_with("ALARM:") {
                    return Err(ControllerError::new("alarm", line));
                }
                // Ignore the startup banner and asynchronous status reports.
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => {
                session.protocol_poisoned = true;
                Err(ControllerError::new(
                    "timeout",
                    "timed out waiting for GRBL acknowledgement; reconnect required",
                ))
            }
        }
    }

    /// Writes a single realtime byte (feed hold, sleep, soft reset, jog
    /// cancel...). Realtime commands do not wait for an ACK; blocking motion
    /// cancellation because the regular protocol is out of sync would be
    /// unsafe.
    pub(super) async fn send_realtime(session: &mut Session, byte: u8) -> CncResult<()> {
        if let Err(error) = session.port.write_all(&[byte]).await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }
        Ok(())
    }
}
