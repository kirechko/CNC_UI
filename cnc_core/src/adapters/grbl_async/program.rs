//! Stop-and-wait program streaming over the serial link plus the host-side
//! program file loading that feeds it.

use super::{GrblAdapter, PendingProgramAck, Session, StreamReply, MAX_PROGRAM_BYTES};
use crate::{CncResult, ControllerError};
use std::path::Path;
use std::time::Instant;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
};

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
        // Validate under the target family's line budget (FluidNC programs can
        // carry longer lines than stock GRBL accepts — task: FluidNC COM).
        let lines = Self::parse_program_lines(&content, self.protocol)?;
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

    /// Advances program streaming by one scheduler step, never blocking on
    /// the planner acknowledgement.
    ///
    /// Strict stop-and-wait flow control is preserved: at most ONE line is in
    /// flight (`session.pending_ack`), and the next line is written only after
    /// GRBL answered the previous one. The crucial difference from the old
    /// blocking implementation is WHERE the answer is awaited: GRBL emits the
    /// `ok` of a motion line only when room appears in the planner buffer —
    /// which can take as long as one slow block (tens of seconds). Blocking on
    /// it inside `status()` froze the whole 10 Hz poll, so coordinates and the
    /// trajectory highlight advanced in multi-second jumps. Instead,
    /// [`Self::query_status`] routes the `ok`/`error:`/`ALARM:` lines it
    /// drains into the pending request, and this pump only *observes* the
    /// recorded reply and acts on it:
    ///
    /// * no reply yet   → return immediately (keep polling, budget-guarded);
    /// * `ok`           → advance the cursor and write the next line now;
    /// * tolerated `error:20` on bare M7/M8/M9 → skip the line;
    /// * any other reply / budget exhaustion → abort streaming with an error.
    ///
    /// Throughput is one line per scheduler pass, which is exactly the old
    /// cadence whenever GRBL acknowledged promptly, and vastly smoother when
    /// it does not.
    ///
    /// `allow_write` gates step 2 (sending the next line) by the machine
    /// state observed in the *current* status report: Hold/Door/Alarm pause
    /// forwarding, while replies of lines already in flight are still
    /// resolved in any state, so an `ALARM:` on the wire aborts the program
    /// even while the controller is not streamable.
    pub(super) async fn pump_program_stream(
        session: &mut Session,
        allow_write: bool,
    ) -> CncResult<()> {
        if !session.program_streaming {
            return Ok(());
        }

        // Step 1: resolve an in-flight line, if any.
        let resolve = match session.pending_ack.as_ref() {
            None => None,
            Some(pending) => match &pending.reply {
                // The answer has not been observed yet: enforce the extended
                // ACK budget, then leave the line in flight.
                None => {
                    if pending.sent_at.elapsed() >= super::PROGRAM_ACK_TIMEOUT {
                        session.pending_ack = None;
                        session.program_streaming = false;
                        session.protocol_poisoned = true;
                        return Err(ControllerError::new(
                            "timeout",
                            "timed out waiting for GRBL program acknowledgement; reconnect required",
                        ));
                    }
                    return Ok(());
                }
                // Reply recorded by the status reader: consume it.
                Some(reply) => Some((pending.line.clone(), reply.clone())),
            },
        };
        if let Some((line, reply)) = resolve {
            session.pending_ack = None;
            match reply {
                StreamReply::Ok => {
                    session.current_line += 1;
                    // Task T12: remember the modal motion group of the
                    // acknowledged line so the status poll can show
                    // G0/G1/G2/G3 instead of the generic "G-code".
                    if let Some(mode) = Self::motion_mode_of(&line) {
                        session.motion_mode = Some(mode.to_string());
                    }
                }
                StreamReply::Error(detail) => {
                    // Rebuild the same error shape the blocking path produced
                    // so the tolerated-accessory rule keeps working verbatim:
                    // unsupported coolant M-codes (stock GRBL without
                    // COOLANT_CONTROL, error 20) are skipped; anything else
                    // aborts the program.
                    let error = ControllerError::new(
                        "gcode_error",
                        format!("GRBL rejected command ({detail}): {line}"),
                    );
                    if !Self::is_tolerated_accessory_rejection(&line, &error) {
                        session.program_streaming = false;
                        return Err(error);
                    }
                    tracing::warn!(
                        "GRBL rejected the optional accessory command `{line}` ({}); skipping it",
                        error.message
                    );
                    session.current_line += 1;
                }
                StreamReply::Alarm(code) => {
                    session.program_streaming = false;
                    return Err(ControllerError::new("alarm", format!("ALARM:{code}")));
                }
            }
        }

        // Step 2: nothing in flight — write the next line (at most one).
        // Machine-state gate: forwarding only continues while the current
        // report says the controller accepts blocks (Idle/Run/Check); Hold,
        // Door, Jog and Alarm pause it exactly like the old blocking loop did
        // (which simply never reached `stream_next_program_line`). Replies of
        // lines already in flight were resolved above regardless of state.
        if !allow_write {
            return Ok(());
        }
        let index = session.current_line as usize;
        if index >= session.program.len() {
            session.program_streaming = false;
            return Ok(());
        }
        let line = session.program[index].clone();
        Self::validate_command(&line)?;
        if let Err(error) = session.port.write_all(line.as_bytes()).await {
            session.protocol_poisoned = true;
            session.program_streaming = false;
            return Err(ControllerError::new("io", error.to_string()));
        }
        if let Err(error) = session.port.write_all(b"\n").await {
            session.protocol_poisoned = true;
            session.program_streaming = false;
            return Err(ControllerError::new("io", error.to_string()));
        }
        session.pending_ack = Some(PendingProgramAck {
            line,
            sent_at: Instant::now(),
            reply: None,
        });
        // Keep streaming active until the program is exhausted:
        // a GRBL ACK means the line was received, not that the motion has finished.
        Ok(())
    }

    /// Upper bound (1-based) of program lines allowed in the forward
    /// progress search for the current physical tool position.
    ///
    /// GRBL emits `ok` only after the ENTIRE line has been enqueued into
    /// the planner. Short lines fit at once, so their acknowledgement
    /// arrives immediately and `current_line` (count of acknowledged lines)
    /// tracks the tool. Long arcs (G2/G3) are different: GRBL decomposes
    /// them into hundreds of tiny chord blocks bounded by `$119` arc
    /// tolerance, and the planner buffer holds only 16 blocks, so writing
    /// an arc line is throttled by the machine actually consuming earlier
    /// chords. The `ok` of a full-circle arc therefore arrives only at
    /// ~80-90% of the physical move.
    ///
    /// While a line is in flight (`has_pending_ack` is `true`), the tool is
    /// already cutting it even though it has not been acknowledged, so its
    /// 1-based number `current_line + 1` must join the search window.
    /// Without this, `locate_forward` excludes the arc chords, every poll
    /// falls back to the frozen `last_progress`, and the traversed part of
    /// the arc jumps in one frame when the delayed `ok` finally arrives.
    ///
    /// Matching remains safe: a segment only wins when the tool lies within
    /// `PATH_TOLERANCE_MM` of it, the search is forward-only (`floor`), and
    /// among candidates the EARLIEST distance is chosen, so the highlight
    /// can never run ahead of the physical tool.
    pub(super) fn progress_line_window(current_line: u32, has_pending_ack: bool) -> u32 {
        if has_pending_ack {
            current_line + 1
        } else {
            current_line
        }
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
