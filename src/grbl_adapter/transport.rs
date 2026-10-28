//! Byte-level serial transport for [`GrblAdapter`]: line reading against a deadline,
//! stop-and-wait command exchange (`ok` / `error:` acknowledgement), realtime byte
//! injection, the bounded continuous-XY-jog chunk writer, and the `?`/`$$` queries.

use super::{
    ContinuousXyJog, GrblAdapter, Session, MAX_RESPONSE_LINE, RESPONSE_TIMEOUT, STATUS_TIMEOUT,
};
use crate::{ControllerError, CncResult};
use serialport::SerialPort;
use std::collections::HashMap;
use std::io::Write;
use std::time::Instant;

impl GrblAdapter {
    /// Reads bytes until a newline (or the deadline) and returns the trimmed line.
    /// Ignores bare `\r` bytes and transient timeout/would-block/interrupted read
    /// errors so a slow GRBL controller still gets a chance to answer in time.
    pub(super) fn read_line(port: &mut dyn SerialPort, deadline: Instant) -> CncResult<String> {
        let mut bytes = Vec::new();

        while Instant::now() < deadline {
            let mut byte = [0_u8; 1];
            match port.read(&mut byte) {
                Ok(0) => continue,
                Ok(_) if byte[0] == b'\n' => {
                    return Ok(String::from_utf8_lossy(&bytes).trim().to_string());
                }
                Ok(_) if byte[0] != b'\r' => {
                    bytes.push(byte[0]);
                    if bytes.len() > MAX_RESPONSE_LINE {
                        return Err(ControllerError::new(
                            "protocol",
                            "GRBL response line exceeds size limit",
                        ));
                    }
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => {
                    return Err(ControllerError::new("io", error.to_string()));
                }
            }
        }

        Err(ControllerError::new("timeout", "timed out waiting for GRBL response"))
    }

    /// Writes one trimmed command line and blocks until GRBL acknowledges it with
    /// `ok`. Startup banners and asynchronous status reports are skipped; `error:`
    /// and `ALARM:` replies become typed controller errors.
    pub(super) fn send_command(session: &mut Session, command: &str) -> CncResult<()> {
        if command.contains('\n') || command.contains('\r') {
            return Err(ControllerError::new(
                "bad_command",
                "a GRBL command must contain exactly one line",
            ));
        }
        if command.trim().is_empty() {
            return Ok(());
        }

        session
            .port
            .write_all(command.trim().as_bytes())
            .and_then(|_| session.port.write_all(b"\n"))
            .and_then(|_| session.port.flush())
            .map_err(|error| ControllerError::new("io", error.to_string()))?;

        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let line = Self::read_line(&mut *session.port, deadline)?;
            if line == "ok" {
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
            // Startup banners and asynchronous status reports are not command ACKs.
        }
    }

    /// Writes a single realtime byte (e.g. `?`, `!`, `~`, 0x18, 0x85) without
    /// waiting for an acknowledgement, as required by the GRBL realtime protocol.
    pub(super) fn send_realtime(session: &mut Session, byte: u8) -> CncResult<()> {
        session
            .port
            .write_all(&[byte])
            .and_then(|_| session.port.flush())
            .map_err(|error| ControllerError::new("io", error.to_string()))
    }

    /// Emits one bounded `$J` jog block so held continuous jogs move in short
    /// increments; GRBL cancels the current block with the realtime command 0x85.
    pub(super) fn send_continuous_xy_chunk(
        session: &mut Session,
        jog: ContinuousXyJog,
    ) -> CncResult<()> {
        const CHUNK_LENGTH_MM: f64 = 10.0;
        let norm = f64::from(
            jog.x_direction * jog.x_direction + jog.y_direction * jog.y_direction,
        )
        .sqrt();
        let x = CHUNK_LENGTH_MM * f64::from(jog.x_direction) / norm;
        let y = CHUNK_LENGTH_MM * f64::from(jog.y_direction) / norm;
        let command = format!(
            "$J=G91 X{x:.4} Y{y:.4} F{:.0}",
            jog.feed_rate
        );
        Self::send_command(session, &command)
    }

    /// Sends the realtime `?` status request and returns the first `<...>` report line.
    pub(super) fn query_status(session: &mut Session) -> CncResult<String> {
        session
            .port
            .write_all(b"?")
            .and_then(|_| session.port.flush())
            .map_err(|error| ControllerError::new("io", error.to_string()))?;

        let deadline = Instant::now() + STATUS_TIMEOUT;
        loop {
            let line = Self::read_line(&mut *session.port, deadline)?;
            if line.starts_with('<') && line.ends_with('>') {
                return Ok(line);
            }
        }
    }

    /// Runs `$$` and collects all `$N=value` settings into a numeric map; values that
    /// do not parse as finite floats are skipped.
    pub(super) fn query_settings(session: &mut Session) -> CncResult<HashMap<u16, f64>> {
        session
            .port
            .write_all(b"$$\n")
            .and_then(|_| session.port.flush())
            .map_err(|error| ControllerError::new("io", error.to_string()))?;

        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        let mut settings = HashMap::new();

        loop {
            let line = Self::read_line(&mut *session.port, deadline)?;
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
    }
}
