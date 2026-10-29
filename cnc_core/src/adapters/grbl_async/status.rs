//! Parsing of the GRBL realtime status report (`<State|MPos:...|WPos:...|...>`)
//! into the neutral [`MachineStatus`] structure used by the whole UI.

use super::GrblAdapter;
use crate::{
    CncResult, ControllerError, CoordinateSystem, InterpreterState, MachineStatus, Position,
    SpindleCommand, SpindleStatus, TaskMode, TaskState, ToolpathProgress,
};

impl GrblAdapter {
    /// Parses a comma-separated `X,Y,Z` triplet, rejecting extra components
    /// and non-finite values.
    pub(super) fn parse_triplet(value: &str) -> Option<[f64; 3]> {
        let mut values = value.split(',').map(str::parse::<f64>);
        let result = [
            values.next()?.ok()?,
            values.next()?.ok()?,
            values.next()?.ok()?,
        ];
        if values.next().is_some() || result.iter().any(|value| !value.is_finite()) {
            return None;
        }
        Some(result)
    }

    /// Parses the combined `FS:feed,spindle` field into two non-negative
    /// finite numbers.
    pub(super) fn parse_feed_spindle(value: &str) -> CncResult<(f64, f64)> {
        let mut values = value.split(',');
        let feed_rate = values
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| ControllerError::new("protocol", "invalid GRBL feed-rate field"))?;
        let spindle_rpm = values
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| ControllerError::new("protocol", "invalid GRBL spindle-speed field"))?;

        if values.next().is_some() {
            return Err(ControllerError::new(
                "protocol",
                "GRBL FS field must contain exactly two values",
            ));
        }

        Ok((feed_rate, spindle_rpm))
    }

    /// Verifies the state token and its optional subcode (`Hold:0`, `Door:1`,
    /// `Alarm:2`) against the values GRBL 1.1 can actually report.
    pub(super) fn is_known_machine_state(state: &str) -> bool {
        let (name, detail) = match state.split_once(':') {
            Some((name, detail)) => (name.to_ascii_lowercase(), Some(detail)),
            None => (state.to_ascii_lowercase(), None),
        };

        match name.as_str() {
            "idle" | "run" | "jog" | "check" | "home" | "sleep" => detail.is_none(),
            "hold" => matches!(detail, Some("0" | "1")),
            "door" => detail
                .and_then(|value| value.parse::<u8>().ok())
                .is_some_and(|state| state <= 3),
            "alarm" => match detail {
                None => true,
                Some(value) => value.parse::<u16>().is_ok_and(|code| code > 0),
            },
            _ => false,
        }
    }

    /// Converts one status report line into a full [`MachineStatus`] snapshot.
    ///
    /// Work coordinates may be reported directly (`WPos`), derived from
    /// `MPos - WCO`, or reconstructed from the offset cached for the active
    /// coordinate system; every combination is validated so a malformed or
    /// overflowing reply becomes a protocol error instead of bogus numbers.
    pub(super) fn parse_status(
        response: &str,
        cached_work_offset: Option<[f64; 3]>,
        active_coordinate_system: CoordinateSystem,
        spindle_command: SpindleCommand,
        program_name: Option<&str>,
        current_line: u32,
        total_lines: u32,
    ) -> CncResult<MachineStatus> {
        let body = response
            .strip_prefix('<')
            .and_then(|value| value.strip_suffix('>'))
            .ok_or_else(|| ControllerError::new("protocol", "malformed GRBL status response"))?;

        let mut fields = body.split('|');
        let state = fields.next().unwrap_or("Unknown");
        let state_lower = state.to_ascii_lowercase();
        if !Self::is_known_machine_state(state) {
            return Err(ControllerError::new(
                "protocol",
                format!("unknown GRBL machine state: {state}"),
            ));
        }

        let mut machine_position = None;
        let mut work_position = None;
        let mut work_offset = None;
        let mut feed_rate = 0.0;
        let mut actual_rpm = 0.0;
        let mut feed_spindle_seen = false;
        let mut feed_seen = false;

        for field in fields {
            if let Some(value) = field.strip_prefix("MPos:") {
                if machine_position.is_some() {
                    return Err(ControllerError::new(
                        "protocol",
                        "duplicate GRBL MPos field",
                    ));
                }
                machine_position =
                    Some(Self::parse_triplet(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL MPos field")
                    })?);
            } else if let Some(value) = field.strip_prefix("WPos:") {
                if work_position.is_some() {
                    return Err(ControllerError::new(
                        "protocol",
                        "duplicate GRBL WPos field",
                    ));
                }
                work_position =
                    Some(Self::parse_triplet(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL WPos field")
                    })?);
            } else if let Some(value) = field.strip_prefix("WCO:") {
                if work_offset.is_some() {
                    return Err(ControllerError::new("protocol", "duplicate GRBL WCO field"));
                }
                work_offset =
                    Some(Self::parse_triplet(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL WCO field")
                    })?);
            } else if let Some(value) = field.strip_prefix("FS:") {
                if feed_seen {
                    return Err(ControllerError::new(
                        "protocol",
                        "duplicate GRBL feed-rate field",
                    ));
                }
                (feed_rate, actual_rpm) = Self::parse_feed_spindle(value)?;
                feed_spindle_seen = true;
                feed_seen = true;
            } else if let Some(value) = field.strip_prefix("F:") {
                if feed_seen || feed_spindle_seen {
                    return Err(ControllerError::new(
                        "protocol",
                        "duplicate GRBL feed-rate field",
                    ));
                }
                feed_rate = value
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL feed-rate field")
                    })?;
                feed_seen = true;
            }
        }

        if machine_position.is_none() && work_position.is_none() {
            return Err(ControllerError::new(
                "protocol",
                "GRBL status response contains no machine or work position",
            ));
        }
        if machine_position.is_none() && work_offset.is_none() && cached_work_offset.is_none() {
            return Err(ControllerError::new(
                "protocol",
                "cannot determine work coordinates without a work coordinate offset",
            ));
        }

        let offset = work_offset
            .or_else(|| match (machine_position, work_position) {
                (Some(machine), Some(work)) => Some([
                    machine[0] - work[0],
                    machine[1] - work[1],
                    machine[2] - work[2],
                ]),
                _ => None,
            })
            .or(cached_work_offset)
            .unwrap_or([0.0; 3]);
        if offset.iter().any(|value| !value.is_finite()) {
            return Err(ControllerError::new(
                "protocol",
                "GRBL status produced a non-finite work coordinate offset",
            ));
        }

        let machine = machine_position
            .or_else(|| {
                work_position.map(|work| {
                    [
                        work[0] + offset[0],
                        work[1] + offset[1],
                        work[2] + offset[2],
                    ]
                })
            })
            .unwrap_or([0.0; 3]);
        if machine.iter().any(|value| !value.is_finite()) {
            return Err(ControllerError::new(
                "protocol",
                "GRBL status produced a non-finite machine position",
            ));
        }

        let alarm = state_lower.starts_with("alarm");
        let running = state_lower.starts_with("run");
        let jogging = state_lower.starts_with("jog");
        // GRBL reports the door interlock as Door:n; surface it as paused
        // instead of presenting an interlocked machine as idle.
        let paused = state_lower.starts_with("hold") || state_lower.starts_with("door");
        let homing = state_lower.starts_with("home");

        Ok(MachineStatus {
            connected: true,
            task_mode: if running {
                TaskMode::Auto
            } else {
                TaskMode::Manual
            },
            task_state: if alarm {
                TaskState::Estop
            } else {
                TaskState::On
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
                a: 0.0,
                b: 0.0,
                c: 0.0,
            },
            work_offset: Position {
                x: offset[0],
                y: offset[1],
                z: offset[2],
                a: 0.0,
                b: 0.0,
                c: 0.0,
            },
            active_coordinate_system,
            coordinate_systems: Vec::new(),
            feed_rate: if jogging { 0.0 } else { feed_rate },
            jog_rate: if jogging { feed_rate } else { 0.0 },
            motion_mode: if jogging {
                Some("Jog".into())
            } else if running {
                Some("G-code".into())
            } else {
                None
            },
            spindle: SpindleStatus {
                commanded_rpm: spindle_command.rpm,
                actual_rpm,
                direction: spindle_command.direction,
                enabled: actual_rpm > 0.0 || spindle_command.rpm > 0.0,
            },
            mist_coolant_on: false,
            flood_coolant_on: false,
            max_spindle_rpm: 12_000.0,
            loaded_program: program_name.map(str::to_owned),
            current_line,
            total_lines,
            message: Some(format!("GRBL {state}")),
            homed: [false; 6],
            homing,
            progress: ToolpathProgress::default(),
        })
    }
}
