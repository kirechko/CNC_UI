//! Stop-and-wait program streaming over the serial link plus the host-side
//! program file loading that feeds it.

use super::{GrblAdapter, Session, MAX_PROGRAM_BYTES};
use crate::{CncResult, ControllerError};
use std::path::Path;
use tokio::{fs, io::AsyncReadExt};

impl GrblAdapter {
    /// Reads a whole G-code file from the host filesystem, validates every
    /// line against the GRBL command grammar and installs the resulting
    /// program (plus its parsed toolpath for progress tracking) into the
    /// session. All file work happens *before* the session lock is taken, so
    /// a bad file never leaves the adapter half-loaded.
    pub(super) async fn load_program_file(&self, path: &Path) -> CncResult<()> {
        let file = fs::File::open(path)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        let mut bytes = Vec::with_capacity(MAX_PROGRAM_BYTES.min(64 * 1024));
        file.take((MAX_PROGRAM_BYTES as u64) + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;

        if bytes.len() > MAX_PROGRAM_BYTES {
            return Err(ControllerError::new(
                "bad_program",
                format!("GRBL program exceeds the {MAX_PROGRAM_BYTES}-byte size limit"),
            ));
        }

        let content = String::from_utf8(bytes)
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        let lines = Self::parse_program_lines(&content)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("program.nc")
            .to_owned();

        let mut session = self.session.lock().await;
        Self::ensure_program_idle(
            session.program_streaming,
            session.continuous_jog.is_some() || session.jog_chunk_active,
        )?;
        session.toolpath = crate::parse_toolpath(&content);
        session.program = lines;
        session.program_name = Some(name);
        session.current_line = 0;
        session.program_streaming = false;
        // A fresh program invalidates any remembered traversed path.
        session.last_progress = None;
        Ok(())
    }

    /// Streaming may only continue while GRBL reports `Idle`, `Run` or
    /// `Check` (dry-run mode); other states (Hold, Door, Alarm, Jog) pause the
    /// scheduler until the operator releases the machine. `Check` must be
    /// included because a GRBL controller that stays in Check mode never
    /// reports `Run` while a program is being verified without motion.
    pub(super) fn should_stream_program(status_response: &str) -> bool {
        status_response
            .strip_prefix('<')
            .and_then(|response| response.strip_suffix('>'))
            .and_then(|response| response.split('|').next())
            .is_some_and(|state| {
                state.eq_ignore_ascii_case("Idle")
                    || state.eq_ignore_ascii_case("Run")
                    || state.eq_ignore_ascii_case("Check")
            })
    }

    /// Sends the next program line and advances the cursor.
    ///
    /// Strict stop-and-wait flow control: over Serial only one line is sent, and the next
    /// send_command call is possible only after GRBL returns an `ok` ACK or an error.
    /// Therefore at most one line is unacknowledged and the RX buffer cannot overflow,
    /// even at 115200+ baud. A fixed delay is not needed and does not replace the controller ACK.
    pub(super) async fn stream_next_program_line(session: &mut Session) -> CncResult<()> {
        if !session.program_streaming {
            return Ok(());
        }

        let index = session.current_line as usize;
        if index >= session.program.len() {
            session.program_streaming = false;
            return Ok(());
        }

        let line = session.program[index].clone();
        // Stop-and-wait over the serial link: GRBL acknowledges a motion line
        // only once room appears in the planner buffer, so the wait can last
        // as long as one buffered block. The extended program ACK budget keeps
        // a slow controller from falsely poisoning the protocol.
        if let Err(error) =
            Self::send_command_with_timeout(session, &line, super::PROGRAM_ACK_TIMEOUT).await
        {
            // Unsupported coolant M-codes (stock GRBL without COOLANT_CONTROL)
            // are skipped so the cut keeps running; everything else aborts.
            if Self::is_tolerated_accessory_rejection(&line, &error) {
                tracing::warn!(
                    "GRBL rejected the optional accessory command `{line}` ({}); skipping it",
                    error.message
                );
                session.current_line = (index + 1) as u32;
                return Ok(());
            }
            session.program_streaming = false;
            return Err(error);
        }

        session.current_line = (index + 1) as u32;
        // Task T12: remember the modal motion group of the acknowledged line
        // so the status poll can show G0/G1/G2/G3 instead of the generic
        // "G-code" (GRBL's '?' report does not expose the modal group).
        if let Some(mode) = Self::motion_mode_of(&line) {
            session.motion_mode = Some(mode.to_string());
        }
        // Keep streaming active until the next Idle response:
        // a GRBL ACK means the line was received, not that the motion has finished.
        Ok(())
    }

    /// Extracts the modal motion group (G0/G1/G2/G3) of one G-code line:
    /// the first explicit G-word with a code in {0, 1, 2, 3}. Lines that only
    /// carry modal continuation (no G-word) return `None` and keep whatever
    /// mode was in effect before. Parenthesis comments are skipped so
    /// `(G1 roughing)` does not register.
    pub(super) fn motion_mode_of(line: &str) -> Option<&'static str> {
        let bytes = line.as_bytes();
        let mut in_comment = false;
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'(' => in_comment = true,
                b')' => in_comment = false,
                b';' if !in_comment => break,
                b'G' | b'g' if !in_comment => {
                    let mut end = index + 1;
                    while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
                        end += 1;
                    }
                    if end > index + 1 {
                        if let Ok(code) = line[index + 1..end].parse::<f64>() {
                            let mode = match code {
                                0.0 => "G0",
                                1.0 => "G1",
                                2.0 => "G2",
                                3.0 => "G3",
                                _ => {
                                    index = end;
                                    continue;
                                }
                            };
                            return Some(mode);
                        }
                    }
                    index = end;
                }
                _ => index += 1,
            }
        }
        None
    }
}
