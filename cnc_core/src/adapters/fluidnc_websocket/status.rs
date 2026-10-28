//! Status-report parsing and the full status collection cycle of
//! [`FluidNcWebSocketAdapter`]. The cycle body was extracted from the
//! `CncApi::status` trait method (kept thin in `api.rs`) so the single trait
//! impl block stays compact; everything here is inherent helpers.

use super::{FluidNcWebSocketAdapter, Session};
use crate::toolpath::ToolpathProgress;
use crate::{
    ControllerError, CncEvent, CncResult, InterpreterState, MachineStatus, Position,
    SpindleStatus, TaskMode,
};

impl FluidNcWebSocketAdapter {
    /// Number of axes implied by the first MPos/WPos field of a status report.
    pub(super) fn reported_axis_count(response: &str) -> Option<usize> {
        response
            .split('|')
            .find(|field| field.starts_with("MPos:") || field.starts_with("WPos:"))
            .and_then(|field| field.split_once(':'))
            .map(|(_, values)| values.split(',').count())
            .filter(|count| (3..=6).contains(count))
    }

    /// Parse a comma-separated axis tuple into a fixed six-slot array.
    fn parse_axes(value: &str) -> Option<[f64; 6]> {
        let mut axes = [0.0_f64; 6];
        let mut count = 0;

        for component in value.split(',') {
            if count == axes.len() {
                return None;
            }
            axes[count] = component.parse().ok()?;
            if !axes[count].is_finite() {
                return None;
            }
            count += 1;
        }

        (count >= 3).then_some(axes)
    }

    /// Convert one raw `<...|...>` FluidNC report into the universal status
    /// snapshot, reconciling MPos/WPos/WCO according to field precedence.
    fn parse_status(session: &Session, response: &str) -> CncResult<MachineStatus> {
        let body = response
            .strip_prefix('<')
            .and_then(|value| value.strip_suffix('>'))
            .ok_or_else(|| ControllerError::new("protocol", "malformed FluidNC status response"))?;

        let mut fields = body.split('|');
        let state = fields.next().unwrap_or("Unknown");
        let mut machine_position: Option<[f64; 6]> = None;
        let mut work_position: Option<[f64; 6]> = None;
        let mut work_offset: Option<[f64; 6]> = None;
        let mut feed_rate = 0.0;
        let mut spindle_rpm = 0.0;

        for field in fields {
            if let Some(value) = field.strip_prefix("MPos:") {
                machine_position = Self::parse_axes(value);
            } else if let Some(value) = field.strip_prefix("WPos:") {
                work_position = Self::parse_axes(value);
            } else if let Some(value) = field.strip_prefix("WCO:") {
                work_offset = Self::parse_axes(value);
            } else if let Some(value) = field.strip_prefix("FS:") {
                let mut values = value.split(',');
                feed_rate = values
                    .next()
                    .and_then(|item| item.parse().ok())
                    .unwrap_or(0.0);
                spindle_rpm = values
                    .next()
                    .and_then(|item| item.parse().ok())
                    .unwrap_or(0.0);
            } else if let Some(value) = field.strip_prefix("F:") {
                feed_rate = value.parse().unwrap_or(0.0);
            }
        }

        let offset = work_offset
            .or_else(|| match (machine_position, work_position) {
                (Some(machine), Some(work)) => {
                    Some(std::array::from_fn(|axis| machine[axis] - work[axis]))
                }
                _ => None,
            })
            .or(session.work_offset)
            .unwrap_or([0.0; 6]);
        let machine = machine_position
            .or_else(|| {
                work_position.map(|work| std::array::from_fn(|axis| work[axis] + offset[axis]))
            })
            .ok_or_else(|| {
                ControllerError::new("protocol", "FluidNC status contains no valid position")
            })?;

        let lower_state = state.to_ascii_lowercase();
        let alarm = lower_state.starts_with("alarm");
        let running = lower_state == "run";
        let jogging = lower_state == "jog";
        let paused = lower_state.starts_with("hold") || lower_state.starts_with("door");

        Ok(MachineStatus {
            connected: true,
            task_mode: if running {
                TaskMode::Auto
            } else {
                TaskMode::Manual
            },
            task_state: if alarm {
                crate::TaskState::Estop
            } else {
                crate::TaskState::On
            },
            interpreter_state: if running {
                InterpreterState::Reading
            } else if paused {
                InterpreterState::Paused
            } else {
                InterpreterState::Idle
            },
            position: Position {
                x: machine[0],
                y: machine[1],
                z: machine[2],
                a: machine[3],
                b: machine[4],
                c: machine[5],
            },
            work_offset: Position {
                x: offset[0],
                y: offset[1],
                z: offset[2],
                a: offset[3],
                b: offset[4],
                c: offset[5],
            },
            active_coordinate_system: session.active_coordinate_system,
            coordinate_systems: Vec::new(),
            feed_rate: if jogging { 0.0 } else { feed_rate },
            spindle: SpindleStatus {
                commanded_rpm: session.spindle_command.rpm,
                actual_rpm: spindle_rpm,
                direction: session.spindle_command.direction,
                enabled: spindle_rpm > 0.0 || session.spindle_command.rpm > 0.0,
            },
            mist_coolant_on: false,
            flood_coolant_on: false,
            jog_rate: if jogging { feed_rate } else { 0.0 },
            motion_mode: if jogging {
                Some("Jog".into())
            } else if running {
                Some("G-code".into())
            } else {
                None
            },
            max_spindle_rpm: 0.0,
            loaded_program: session.program_name.clone(),
            current_line: session.current_line,
            total_lines: session.program.len() as u32,
            message: Some(format!("FluidNC {state}")),
            homed: [false; 6],
            homing: lower_state == "home",
            progress: ToolpathProgress::default(),
        })
    }

    /// Perform one full status cycle: connect if needed, poll the controller,
    /// advance streaming bookkeeping, keep continuous jog alive while idle,
    /// derive toolpath progress and publish the resulting snapshot.
    pub(super) async fn collect_status(&self) -> CncResult<MachineStatus> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let response = self.query_status(&mut session).await?;
        self.status_timeouts
            .store(0, std::sync::atomic::Ordering::SeqCst);

        let mut status = Self::parse_status(&session, &response)?;
        if let Some(count) = Self::reported_axis_count(&response) {
            session.reported_axis_count = count;
        }

        let machine_state = response
            .strip_prefix('<')
            .and_then(|value| value.split('|').next())
            .unwrap_or_default();
        let idle = machine_state.eq_ignore_ascii_case("Idle");

        let was_program_streaming = session.program_streaming;
        if session.program_streaming
            && idle
            && session.current_line as usize >= session.program.len()
            && session.program_acknowledgements.is_empty()
        {
            session.program_streaming = false;
        } else if idle && !session.program_streaming {
            if let Some(jog) = session.continuous_jog {
                self.send_jog_chunk(&mut session, jog).await?;
            }
        }

        if let Some(error) = session.program_error.take() {
            status.message = Some(error);
        }
        status.current_line = session
            .active_program_line
            .map_or(session.current_line, |line| line + 1);
        // Derive universal progress from the tool position in the work frame (G54).
        // FluidNC reports machine position (MPos); the toolpath is stored in work
        // coordinates, so the work offset (WCO) must be subtracted first.
        // FluidNC acknowledges lines when they enter its motion planner, so
        // `current_line` runs ahead of the physical tool. The search is therefore
        // forward-only: it starts from the last reported distance and only considers
        // lines already sent, so a lift cannot match an earlier pass in the same column.
        let work_position = Position {
            x: status.position.x - status.work_offset.x,
            y: status.position.y - status.work_offset.y,
            z: status.position.z - status.work_offset.z,
            a: status.position.a - status.work_offset.a,
            b: status.position.b - status.work_offset.b,
            c: status.position.c - status.work_offset.c,
        };
        status.progress = if session.program_streaming {
            let floor = session
                .last_progress
                .and_then(|progress| progress.path_distance_mm)
                .unwrap_or(0.0);
            match session
                .toolpath
                .locate_forward(&work_position, floor, session.current_line)
            {
                Some(found) => {
                    session.last_progress = Some(found);
                    found
                }
                // No match in the window: keep the last reported part, update only the tool marker.
                None => session.last_progress.map_or_else(
                    ToolpathProgress::unavailable,
                    |last| ToolpathProgress {
                        tool_position: Some(work_position.clone()),
                        ..last
                    },
                ),
            }
        } else {
            session.last_progress = None;
            ToolpathProgress::unavailable()
        };
        if session.active_program_line.is_some() {
            status.task_mode = TaskMode::Auto;
            if status.interpreter_state == InterpreterState::Idle {
                status.interpreter_state = InterpreterState::Reading;
            }
        }

        if response.contains("|WCO:")
            || (response.contains("|MPos:") && response.contains("|WPos:"))
        {
            session.work_offset = Some([
                status.work_offset.x,
                status.work_offset.y,
                status.work_offset.z,
                status.work_offset.a,
                status.work_offset.b,
                status.work_offset.c,
            ]);
        }
        if was_program_streaming && !session.program_streaming {
            let _ = self.events.send(CncEvent::ProgramFinished);
        }
        self.publish_status(status.clone());
        Ok(status)
    }
}
