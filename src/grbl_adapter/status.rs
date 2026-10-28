//! GRBL status-report parsing and the small formatting helpers (`G5x` numbers,
//! axis words, XYZ triplets) that jog/zeroing commands are built from.

use super::{GrblAdapter, Session};
use crate::{
    Axis, ControllerError, CncResult, CoordinateSystem, InterpreterState, MachineStatus, Position,
    SpindleStatus, TaskMode, TaskState,
};

impl GrblAdapter {
    /// Maps a coordinate system to the 1-based `P` parameter used by `G10 L20`.
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

    /// Formats a single axis word (`X1.234`) for jog and `G10` command assembly.
    pub(super) fn format_axis(axis: Axis, value: f64) -> String {
        let letter = match axis {
            Axis::X => 'X',
            Axis::Y => 'Y',
            Axis::Z => 'Z',
            Axis::A => 'A',
            Axis::B => 'B',
        };
        format!("{letter}{value:.3}")
    }

    /// Parses a comma-separated `x,y,z` triple from a GRBL status field; returns
    /// `None` unless exactly three finite numbers are present.
    pub(super) fn parse_triplet(value: &str) -> Option<[f64; 3]> {
        let mut values = value.split(',').map(str::parse::<f64>);
        let triplet = [
            values.next()?.ok()?,
            values.next()?.ok()?,
            values.next()?.ok()?,
        ];
        if values.next().is_some() || triplet.iter().any(|value| !value.is_finite()) {
            return None;
        }
        Some(triplet)
    }

    /// Turns one `<State|MPos:...|WPos:...|WCO:...|FS:...>` report into a full
    /// [`MachineStatus`], reconciling machine/work positions against the work
    /// offset and deriving task/interpreter states from the GRBL state word.
    pub(super) fn parse_status(
        response: &str,
        session: &Session,
    ) -> CncResult<MachineStatus> {
        let body = response
            .strip_prefix('<')
            .and_then(|value| value.strip_suffix('>'))
            .ok_or_else(|| ControllerError::new("protocol", "malformed GRBL status response"))?;

        let mut fields = body.split('|');
        let state = fields.next().unwrap_or("Unknown");
        let mut machine_position = None;
        let mut work_position = None;
        let mut work_offset = None;
        let mut actual_rpm = 0.0;
        let mut feed_rate = 0.0;

        for field in fields {
            if let Some(value) = field.strip_prefix("MPos:") {
                machine_position = Self::parse_triplet(value);
            } else if let Some(value) = field.strip_prefix("WPos:") {
                work_position = Self::parse_triplet(value);
            } else if let Some(value) = field.strip_prefix("WCO:") {
                work_offset = Self::parse_triplet(value);
            } else if let Some(value) = field.strip_prefix("FS:") {
                let mut values = value.split(',');
                feed_rate = values
                    .next()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0);
                actual_rpm = values
                    .next()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0);
            }
        }

        let offset = work_offset.unwrap_or([0.0; 3]);
        let machine = machine_position
            .or_else(|| work_position.map(|work| [work[0] + offset[0], work[1] + offset[1], work[2] + offset[2]]))
            .unwrap_or([0.0; 3]);
        let work = work_position.unwrap_or([
            machine[0] - offset[0],
            machine[1] - offset[1],
            machine[2] - offset[2],
        ]);

        let state_lower = state.to_ascii_lowercase();
        let is_alarm = state_lower.starts_with("alarm");
        let is_running = state_lower.starts_with("run");
        let is_jogging = state_lower.starts_with("jog");
        let is_paused = state_lower.starts_with("hold");
        let homing = state_lower.starts_with("home");

        Ok(MachineStatus {
            connected: true,
            task_mode: if is_running {
                TaskMode::Auto
            } else {
                TaskMode::Manual
            },
            task_state: if is_alarm { TaskState::Estop } else { TaskState::On },
            interpreter_state: if is_running {
                InterpreterState::Reading
            } else if is_paused {
                InterpreterState::Paused
            } else {
                InterpreterState::Idle
            },
            position: Position {
                x: machine[0],
                y: machine[1],
                z: machine[2],
                a: 0.0,
                b: 0.0,
            },
            work_offset: Position {
                x: offset[0],
                y: offset[1],
                z: offset[2],
                a: 0.0,
                b: 0.0,
            },
            active_coordinate_system: session.active_coordinate_system,
            coordinate_systems: Vec::new(),
            feed_rate: if is_jogging { 0.0 } else { feed_rate },
            jog_rate: if is_jogging { feed_rate } else { 0.0 },
            motion_mode: if is_jogging {
                Some("Jog".into())
            } else if is_running {
                Some("G-code".into())
            } else {
                None
            },
            spindle: SpindleStatus {
                commanded_rpm: session.spindle_command.rpm,
                actual_rpm,
                direction: session.spindle_command.direction,
                enabled: actual_rpm > 0.0 || session.spindle_command.rpm > 0.0,
            },
            mist_coolant_on: false,
            flood_coolant_on: false,
            max_spindle_rpm: 12_000.0,
            loaded_program: session.program_name.clone(),
            current_line: session.current_line,
            total_lines: session.program.len() as u32,
            message: Some(format!("GRBL {state}; work position {:.3}, {:.3}, {:.3}", work[0], work[1], work[2])),
            homed: [false; 5],
            homing,
        })
    }
}
