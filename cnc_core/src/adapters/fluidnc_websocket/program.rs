//! Program handling for [`FluidNcWebSocketAdapter`]: buffered line streaming
//! with acknowledgement window bookkeeping, host-file and inline program
//! loading, plus the coordinate-system/axis command formatting helpers.
//! The `CncApi` trait implementation lives in `api.rs`.

use super::{
    normalized_program_lines, FluidNcWebSocketAdapter, FluidNcWebSocketConfig, Session,
    MAX_PROGRAM_BYTES,
};
use crate::{CncResult, ControllerError, CoordinateSystem};
use std::path::Path;

impl FluidNcWebSocketAdapter {
    /// Map a work coordinate system to its 1-based `G10 P` parameter number.
    pub(super) fn coordinate_number(system: CoordinateSystem) -> u8 {
        match system {
            CoordinateSystem::G54 => 1,
            CoordinateSystem::G55 => 2,
            CoordinateSystem::G56 => 3,
            CoordinateSystem::G57 => 4,
            CoordinateSystem::G58 => 5,
            CoordinateSystem::G59 => 6,
            CoordinateSystem::G59_1 => 7,
            CoordinateSystem::G59_2 => 8,
            CoordinateSystem::G59_3 => 9,
        }
    }

    /// Map a logical axis to its FluidNC letter.
    pub(super) fn axis_letter(axis: crate::Axis) -> char {
        match axis {
            crate::Axis::X => 'X',
            crate::Axis::Y => 'Y',
            crate::Axis::Z => 'Z',
            crate::Axis::A => 'A',
            crate::Axis::B => 'B',
            crate::Axis::C => 'C',
        }
    }

    /// Detect a coordinate-system selection word (`G54`...`G59.3`) inside a command.
    pub(super) fn coordinate_system_select(command: &str) -> Option<CoordinateSystem> {
        let command = command.to_ascii_uppercase();
        CoordinateSystem::ALL.into_iter().find(|system| {
            command
                .split_whitespace()
                .any(|word| word == system.label())
        })
    }

    /// Number of bytes one program line occupies on the wire (UTF-8 + newline).
    pub(super) fn program_line_wire_bytes(line: &str) -> usize {
        line.len().saturating_add(1)
    }

    /// Send the next buffered program line if it fits both the controller
    /// line-length limit and the local unacknowledged-bytes window.
    async fn send_next_program_line(
        &self,
        session: &mut Session,
        config: &FluidNcWebSocketConfig,
    ) -> CncResult<()> {
        if !session.program_streaming {
            return Ok(());
        }

        let index = session.current_line as usize;
        if index >= session.program.len() {
            return Ok(());
        }

        let line = session.program[index].clone();
        let wire_bytes = Self::program_line_wire_bytes(&line);
        if line.len() > config.max_line_len {
            return Err(ControllerError::new(
                "protocol",
                format!(
                    "FluidNC program line {} exceeds configured maximum length",
                    index + 1
                ),
            ));
        }
        if session
            .program_acknowledged_bytes
            .saturating_add(wire_bytes)
            > config.rx_buffer_bytes
        {
            return Ok(());
        }
        if let Err(error) = self.send_text(session, &line).await {
            session.program_streaming = false;
            session.active_program_line = None;
            session.program_acknowledgements.clear();
            session.program_acknowledged_bytes = 0;
            session.program_error = Some(format!("{}: {}", error.code, error.message));
            return Err(error);
        }
        Self::trace_program_event("sent", index as u32 + 1, "program line sent");

        session
            .program_acknowledgements
            .push_back((index as u32, wire_bytes));
        session.program_acknowledged_bytes = session
            .program_acknowledged_bytes
            .saturating_add(wire_bytes);
        session.active_program_line = session
            .program_acknowledgements
            .front()
            .map(|(line, _)| *line);
        session.current_line = (index + 1) as u32;
        Ok(())
    }

    /// Fill the unacknowledged-bytes window with as many program lines as possible.
    pub(super) async fn fill_program_window(&self, session: &mut Session) -> CncResult<()> {
        while session.program_streaming
            && (session.current_line as usize) < session.program.len()
            && session.program_acknowledged_bytes < self.config.rx_buffer_bytes
        {
            let line_before = session.current_line;
            self.send_next_program_line(session, &self.config).await?;
            if session.current_line == line_before {
                break;
            }
        }
        Ok(())
    }

    /// Account for one controller reply line while a program is streaming:
    /// `ok` releases the oldest unacknowledged line, `error:`/`ALARM:` stop
    /// streaming and park the message for the next status snapshot.
    pub(super) fn record_program_response(session: &mut Session, line: &str) {
        if session.program_acknowledgements.is_empty() {
            return;
        }

        if line == "ok" {
            if let Some((program_line, byte_count)) = session.program_acknowledgements.pop_front() {
                session.program_acknowledged_bytes = session
                    .program_acknowledged_bytes
                    .saturating_sub(byte_count);
                Self::trace_program_event("ack", program_line + 1, "response=ok");
            }
            session.active_program_line = session
                .program_acknowledgements
                .front()
                .map(|(line, _)| *line);
        } else if let Some(error) = line.strip_prefix("error:") {
            if let Some((program_line, _)) = session.program_acknowledgements.front() {
                Self::trace_program_event(
                    "rejected",
                    *program_line + 1,
                    "controller rejected program line",
                );
            }
            session.program_error = Some(format!("FluidNC rejected a program line: error:{error}"));
            session.program_streaming = false;
            session.active_program_line = None;
            session.program_acknowledgements.clear();
            session.program_acknowledged_bytes = 0;
        } else if line.starts_with("ALARM:") {
            session.program_error = Some(format!("FluidNC reported an alarm: {line}"));
            session.program_streaming = false;
            session.active_program_line = None;
            session.program_acknowledgements.clear();
            session.program_acknowledged_bytes = 0;
        }
    }

    /// Read a host G-code file and install it as the current program.
    /// Body of the `CncApi::load_program` trait method.
    pub(super) async fn load_program_file(&self, path: &Path) -> CncResult<()> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        if content.len() > MAX_PROGRAM_BYTES {
            return Err(ControllerError::new(
                "bad_program",
                format!("FluidNC program exceeds the {MAX_PROGRAM_BYTES}-byte size limit"),
            ));
        }

        let lines = normalized_program_lines(&content);
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("program.nc")
            .to_owned();

        let mut session = self.session.lock().await;
        session.program = lines;
        session.toolpath = crate::parse_toolpath(&content);
        // A newly loaded program starts a fresh forward-only progress search.
        session.last_progress = None;
        session.program_name = Some(name);
        session.current_line = 0;
        session.active_program_line = None;
        session.program_acknowledgements.clear();
        session.program_acknowledged_bytes = 0;
        session.program_error = None;
        session.program_streaming = false;
        Ok(())
    }

    /// Install an editor-provided program (no file on the host) after checking
    /// the size and per-line length limits. Body of the inline branch of
    /// `CncApi::load_program_source`.
    pub(super) async fn load_inline_program(&self, content: String) -> CncResult<()> {
        if content.len() > MAX_PROGRAM_BYTES {
            return Err(ControllerError::new(
                "bad_program",
                format!("FluidNC program exceeds the {MAX_PROGRAM_BYTES}-byte size limit"),
            ));
        }
        let lines = normalized_program_lines(&content);
        for (index, line) in lines.iter().enumerate() {
            if line.len() > self.config.max_line_len {
                return Err(ControllerError::new(
                    "bad_program",
                    format!(
                        "FluidNC program line {} exceeds configured maximum length",
                        index + 1
                    ),
                ));
            }
        }
        let mut session = self.session.lock().await;
        session.program = lines;
        session.program_name = Some("inline.gcode".into());
        session.current_line = 0;
        session.active_program_line = None;
        session.program_acknowledgements.clear();
        session.program_acknowledged_bytes = 0;
        session.program_error = None;
        session.program_streaming = false;
        Ok(())
    }
}
