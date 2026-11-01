//! Parsing of the GRBL realtime status report (`<State|MPos:...|WPos:...|...>`)
//! into the neutral [`MachineStatus`] structure used by the whole UI.

use super::GrblAdapter;
use crate::{
    CncResult, ControllerError, CoordinateSystem, InterpreterState, MachineStatus, Position,
    SpindleCommand, SpindleStatus, TaskMode, TaskState, ToolpathProgress,
};

impl GrblAdapter {
    /// Parses a comma-separated position tuple `X,Y,Z[,A[,B[,C]]]` into a
    /// six-slot array with the trailing axes zero-filled.
    ///
    /// Stock GRBL always reports exactly three components; FluidNC firmware —
    /// which this same serial driver serves (task: FluidNC over COM port) —
    /// reports one component per configured axis, up to six (X/Y/Z/A/B/C).
    /// Tuples shorter than three or longer than six components, non-numeric
    /// parts or non-finite values are rejected.
    pub(super) fn parse_position_tuple(value: &str) -> Option<[f64; 6]> {
        let mut result = [0.0_f64; 6];
        let mut seen = 0_usize;
        for part in value.split(',') {
            if seen == 6 {
                return None;
            }
            let parsed: f64 = part.trim().parse().ok()?;
            if !parsed.is_finite() {
                return None;
            }
            result[seen] = parsed;
            seen += 1;
        }
        (seen >= 3).then_some(result)
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

    /// Number of axes implied by the first `MPos:`/`WPos:` field of a
    /// realtime report: GRBL always sends three components, serial FluidNC
    /// sends one per configured axis up to six (task: FluidNC over a COM
    /// port). `None` when the report carries no position field yet.
    pub(super) fn reported_axis_count(status_response: &str) -> Option<usize> {
        let body = status_response.strip_prefix('<')?.strip_suffix('>')?;
        body.split('|').find_map(|field| {
            let value = field
                .strip_prefix("MPos:")
                .or_else(|| field.strip_prefix("WPos:"))?;
            let count = value.split(',').count();
            (count >= 3 && count <= 6).then_some(count)
        })
    }

    /// Converts one status report line into a full [`MachineStatus`] snapshot.
    ///
    /// Work coordinates may be reported directly (`WPos`), derived from
    /// `MPos - WCO`, or reconstructed from the offset cached for the active
    /// coordinate system; every combination is validated so a malformed or
    /// overflowing reply becomes a protocol error instead of bogus numbers.
    ///
    /// Position fields carry three components on GRBL and up to six on
    /// FluidNC (the same driver serves both — task: FluidNC over COM port);
    /// `cached_work_offset` mirrors the six-slot layout, older GRBL caches
    /// simply keeping the trailing axes zero.
    pub(super) fn parse_status(
        response: &str,
        cached_work_offset: Option<[f64; 6]>,
        active_coordinate_system: CoordinateSystem,
        spindle_command: SpindleCommand,
        spindle_mode: crate::SpindleMode,
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

        // Positions travel as six-slot arrays (X,Y,Z,A,B,C): GRBL fills only
        // the first three, serial FluidNC reports up to six (task: FluidNC
        // over COM port), so rotary coordinates must survive the parse.
        let mut machine_position: Option<[f64; 6]> = None;
        let mut work_position: Option<[f64; 6]> = None;
        let mut work_offset: Option<[f64; 6]> = None;
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
                    Some(Self::parse_position_tuple(value).ok_or_else(|| {
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
                    Some(Self::parse_position_tuple(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL WPos field")
                    })?);
            } else if let Some(value) = field.strip_prefix("WCO:") {
                if work_offset.is_some() {
                    return Err(ControllerError::new("protocol", "duplicate GRBL WCO field"));
                }
                work_offset =
                    Some(Self::parse_position_tuple(value).ok_or_else(|| {
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

        // All derive math is element-wise over six slots: GRBL's trailing
        // rotary slots are zero, so the three-axis case behaves exactly as the
        // old triplet math, while FluidNC rotary offsets survive.
        let offset = work_offset
            .or_else(|| match (machine_position, work_position) {
                (Some(machine), Some(work)) => {
                    let mut delta = [0.0_f64; 6];
                    for (out, (m, w)) in delta.iter_mut().zip(machine.iter().zip(work.iter())) {
                        *out = m - w;
                    }
                    Some(delta)
                }
                _ => None,
            })
            .or(cached_work_offset)
            .unwrap_or([0.0; 6]);
        if offset.iter().any(|value| !value.is_finite()) {
            return Err(ControllerError::new(
                "protocol",
                "GRBL status produced a non-finite work coordinate offset",
            ));
        }

        let machine = machine_position
            .or_else(|| {
                work_position.map(|work| {
                    let mut sum = [0.0_f64; 6];
                    for (out, (w, o)) in sum.iter_mut().zip(work.iter().zip(offset.iter())) {
                        *out = w + o;
                    }
                    sum
                })
            })
            .unwrap_or([0.0; 6]);
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
            active_coordinate_system,
            coordinate_systems: Vec::new(),
            feed_rate: if jogging { 0.0 } else { feed_rate },
            jog_rate: if jogging { feed_rate } else { 0.0 },
            // The precise modal group (G0/G1/G2/G3) is unknown from the '?'
            // report; the scheduler overwrites this with the remembered motion
            // word while a program streams (task T12 in `status()`).
            motion_mode: if jogging {
                Some("Jog".into())
            } else if homing {
                Some("Homing".into())
            } else if running {
                Some("G-code".into())
            } else {
                None
            },
            spindle: SpindleStatus {
                commanded_rpm: spindle_command.rpm,
                actual_rpm,
                direction: if spindle_command.direction == crate::SpindleDirection::Stopped {
                    crate::SpindleDirection::Stopped
                } else {
                    crate::SpindleDirection::Clockwise
                },
                enabled: actual_rpm > 0.0 || spindle_command.rpm > 0.0,
                mode: spindle_mode,
                laser_power: (spindle_mode != crate::SpindleMode::Standard)
                    .then_some(spindle_command.rpm),
            },
            mist_coolant_on: false,
            flood_coolant_on: false,
            max_spindle_rpm: 12_000.0,
            probe_triggered: false,
            probe_position: None,
            selected_tool: None,
            current_tool: None,
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
