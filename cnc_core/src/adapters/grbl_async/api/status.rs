//! Status polling and scheduler work exposed by the GRBL API facade.

use super::super::GrblAdapter;
use crate::{CncResult, InterpreterState, MachineStatus, Position, TaskMode, ToolpathProgress};

impl GrblAdapter {
    /// Queries one realtime status report and, as a side effect, runs the
    /// scheduler: streams the next program line when streaming is active, or
    /// replays the next continuous-jog chunk when the controller is Idle.
    pub(super) async fn api_status(&self) -> CncResult<MachineStatus> {
        let mut session = self.session.lock().await;
        let response = Self::query_status(&mut session).await?;

        // Streaming scheduler pass. The pump resolves any in-flight line
        // acknowledgement that `query_status` routed from the wire in ANY
        // machine state (a late `ok`/`error:`/`ALARM:` must never be lost),
        // but only writes the next line while the controller reports
        // Idle/Run/Check. Nothing here blocks: the old stop-and-wait send
        // inside status() froze the whole poll for up to the planner-ack
        // budget, which is what made the trajectory and coordinates advance
        // in multi-second jumps on GRBL.
        if session.program_streaming {
            let allow_write = Self::should_stream_program(&response);
            Self::pump_program_stream(&mut session, allow_write).await?;
        }

        if Self::is_idle_status(&response) {
            session.jog_chunk_active = false;
            if !session.program_streaming {
                if let Some(jog) = session.continuous_jog {
                    if let Err(error) = Self::send_jog_chunk(&mut session, jog).await {
                        session.continuous_jog = None;
                        return Err(error);
                    }
                    session.jog_chunk_active = true;
                }
            }
        }

        let active_coordinate_system = session.active_coordinate_system;
        let mut status = Self::parse_status(
            &response,
            session.work_offsets.get(&active_coordinate_system).copied(),
            active_coordinate_system,
            session.spindle_command,
            session.spindle_mode,
            session.program_name.as_deref(),
            session.current_line,
            session.program.len() as u32,
        )?;
        // Task T12: replace the generic "G-code" motion mode the parser emits
        // for the Run state with the precise modal group (G0/G1/G2/G3) the
        // adapter last acknowledged into the planner. GRBL's '?' report does
        // not carry it; the streamer/MDI remember what they sent.
        if status.motion_mode.as_deref() == Some("G-code") {
            if let Some(mode) = session.motion_mode.clone() {
                status.motion_mode = Some(mode);
            }
        }
        status.max_spindle_rpm = session.max_spindle_rpm.unwrap_or(0.0);
        // Task: toolpath display for GRBL. The toolpath is parsed from the
        // program text, i.e. it lives in the WORK frame of the active
        // coordinate system, while the parsed status carries the MACHINE
        // position. Locating the machine position on a work-frame path fails
        // whenever the work offset is non-zero, so the traversed part of the
        // path never highlighted. Subtract the work offset first, exactly
        // like the FluidNC WebSocket adapter does.
        let work_position = Position {
            x: status.position.x - status.work_offset.x,
            y: status.position.y - status.work_offset.y,
            z: status.position.z - status.work_offset.z,
            a: status.position.a - status.work_offset.a,
            b: status.position.b - status.work_offset.b,
            c: status.position.c - status.work_offset.c,
        };
        // GRBL acknowledges a line when it ENTERS the planner buffer, but the
        // answer for an arc block (G2/G3) is deferred until the WHOLE arc has
        // been converted into planner segments and accepted: GRBL segments
        // arcs internally (~$ARC_TOLERANCE per chord) and only drains its
        // 16-block planner while the machine executes it. On a long arc the
        // `ok` can therefore arrive at ~75-80% of the physical block, with
        // the tool already far inside the line. A plain `locate` would jump
        // the highlight onto later passes crossing the same coordinates, so
        // the search stays forward-only and monotonic (floor = last reported
        // distance), but its upper bound must include the in-flight line:
        // the line written to GRBL whose `ok` has not been observed yet is
        // exactly the line the tool is cutting. Excluding it froze the arc
        // highlight until the delayed acknowledgement arrived (the 80% jump).
        status.progress = if session.program_streaming {
            let floor = session
                .last_progress
                .and_then(|progress| progress.path_distance_mm)
                .unwrap_or(0.0);
            let max_program_line =
                Self::progress_line_window(session.current_line, session.pending_ack.is_some());
            match session
                .toolpath
                .locate_forward(&work_position, floor, max_program_line)
            {
                Some(found) => {
                    session.last_progress = Some(found);
                    found
                }
                // No match in the window: keep the last reported part and only
                // move the tool marker.
                None => session
                    .last_progress
                    .map_or_else(ToolpathProgress::unavailable, |last| ToolpathProgress {
                        tool_position: Some(work_position.clone()),
                        ..last
                    }),
            }
        } else {
            session.last_progress = None;
            ToolpathProgress::unavailable()
        };
        if session.program_streaming && status.interpreter_state == InterpreterState::Idle {
            status.task_mode = TaskMode::Auto;
            status.interpreter_state = InterpreterState::Reading;
            status.motion_mode = Some("G-code".into());
        }

        if response.contains("|WCO:")
            || (response.contains("|MPos:") && response.contains("|WPos:"))
        {
            session.work_offsets.insert(
                active_coordinate_system,
                [
                    status.work_offset.x,
                    status.work_offset.y,
                    status.work_offset.z,
                    status.work_offset.a,
                    status.work_offset.b,
                    status.work_offset.c,
                ],
            );
        }
        // Remember how many position components the controller actually sends
        // so `machine_configuration` reports exactly the configured axes
        // (serial FluidNC: 3..6 axes — task: FluidNC over a COM port).
        if let Some(count) = Self::reported_axis_count(&response) {
            session.reported_axis_count = count;
        }

        Ok(status)
    }
}
