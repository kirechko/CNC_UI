//! Stop-and-wait program streaming over the serial link plus the host-side
//! program file loading that feeds it.

use super::{GrblAdapter, Session, MAX_PROGRAM_BYTES};
use crate::{ControllerError, CncResult};
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
        Ok(())
    }

    /// Streaming may only continue while GRBL reports `Idle` or `Run`; other
    /// states (Hold, Door, Alarm, Jog) pause the scheduler until the operator
    /// releases the machine.
    pub(super) fn should_stream_program(status_response: &str) -> bool {
        status_response
            .strip_prefix('<')
            .and_then(|response| response.strip_suffix('>'))
            .and_then(|response| response.split('|').next())
            .is_some_and(|state| {
                state.eq_ignore_ascii_case("Idle") || state.eq_ignore_ascii_case("Run")
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
        if let Err(error) = Self::send_command(session, &line).await {
            session.program_streaming = false;
            return Err(error);
        }

        session.current_line = (index + 1) as u32;
        // Keep streaming active until the next Idle response:
        // a GRBL ACK means the line was received, not that the motion has finished.
        Ok(())
    }
}
