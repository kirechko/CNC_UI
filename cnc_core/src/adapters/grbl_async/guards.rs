//! Pure validation and guard helpers: everything that decides whether an
//! operation is supported by GRBL 1.1 or allowed by the current motion state.
//! None of these functions touch the serial port.

use super::GrblAdapter;
use crate::{Axis, ControllerError, CncResult, CoordinateSystem};

impl GrblAdapter {
    /// GRBL 1.1 only moves linear X/Y/Z; rotary axes must be rejected early.
    pub(super) fn ensure_supported_axis(axis: Axis) -> CncResult<()> {
        match axis {
            Axis::X | Axis::Y | Axis::Z => Ok(()),
            Axis::A | Axis::B | Axis::C => Err(ControllerError::new(
                "unsupported_axis",
                format!("GRBL 1.1 does not support the {axis:?} rotary axis"),
            )),
        }
    }

    /// GRBL 1.1 exposes G54-G59 but none of the extended G59.n systems.
    pub(super) fn ensure_supported_coordinate_system(system: CoordinateSystem) -> CncResult<()> {
        match system {
            CoordinateSystem::G54
            | CoordinateSystem::G55
            | CoordinateSystem::G56
            | CoordinateSystem::G57
            | CoordinateSystem::G58
            | CoordinateSystem::G59 => Ok(()),
            CoordinateSystem::G59_1 | CoordinateSystem::G59_2 | CoordinateSystem::G59_3 => {
                Err(ControllerError::new(
                    "unsupported_coordinate_system",
                    format!("GRBL 1.1 does not support {}", system.label()),
                ))
            }
        }
    }

    /// Letter word used in G-code for the axis (X, Y, Z, A, B, C).
    pub(super) fn axis_letter(axis: Axis) -> char {
        match axis {
            Axis::X => 'X',
            Axis::Y => 'Y',
            Axis::Z => 'Z',
            Axis::A => 'A',
            Axis::B => 'B',
            Axis::C => 'C',
        }
    }

    /// Manual (UI-initiated) commands must not interleave with streaming.
    pub(super) fn ensure_manual_command_allowed(program_streaming: bool) -> CncResult<()> {
        if program_streaming {
            return Err(ControllerError::new(
                "motion_busy",
                "cannot issue a manual command while a program is streaming",
            ));
        }
        Ok(())
    }

    /// True when the report's state token is exactly `Idle` (case-insensitive).
    pub(super) fn is_idle_status(status_response: &str) -> bool {
        status_response
            .strip_prefix('<')
            .and_then(|response| response.strip_suffix('>'))
            .and_then(|response| response.split('|').next())
            .is_some_and(|state| state.eq_ignore_ascii_case("Idle"))
    }

    /// Program start is only safe from the Idle state; any other *valid*
    /// state is refused with `motion_busy`, malformed ones with `protocol`.
    pub(super) fn ensure_controller_idle(status_response: &str) -> CncResult<()> {
        let state = status_response
            .strip_prefix('<')
            .and_then(|response| response.strip_suffix('>'))
            .and_then(|response| response.split('|').next())
            .ok_or_else(|| ControllerError::new("protocol", "malformed GRBL status response"))?;

        if !Self::is_known_machine_state(state) {
            return Err(ControllerError::new(
                "protocol",
                format!("unknown GRBL machine state: {state}"),
            ));
        }
        if !state.eq_ignore_ascii_case("Idle") {
            return Err(ControllerError::new(
                "motion_busy",
                format!("cannot start a program while GRBL state is {state}"),
            ));
        }

        Ok(())
    }

    /// A continuous jog owns the motion queue; refuse unrelated commands.
    pub(super) fn ensure_jog_idle(jog_active: bool) -> CncResult<()> {
        if jog_active {
            return Err(ControllerError::new(
                "motion_busy",
                "cannot issue this command while a continuous Jog is active",
            ));
        }
        Ok(())
    }

    /// Loading or starting a program requires both streaming and jog to be off.
    pub(super) fn ensure_program_idle(program_streaming: bool, jog_active: bool) -> CncResult<()> {
        if program_streaming || jog_active {
            return Err(ControllerError::new(
                "motion_busy",
                "cannot load or start a program while another program or Jog is active",
            ));
        }
        Ok(())
    }

    /// Validates the whole program before any line is sent: size limit plus
    /// per-line [`Self::validate_command`], reporting the offending line.
    pub(super) fn parse_program_lines(content: &str) -> CncResult<Vec<String>> {
        if content.len() > super::MAX_PROGRAM_BYTES {
            return Err(ControllerError::new(
                "bad_program",
                format!(
                    "GRBL program exceeds the {}-byte size limit",
                    super::MAX_PROGRAM_BYTES
                ),
            ));
        }

        let mut lines = Vec::new();

        for (line_index, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            Self::validate_command(line).map_err(|error| {
                ControllerError::new(
                    "bad_program",
                    format!(
                        "invalid GRBL program line {}: {}",
                        line_index + 1,
                        error.message
                    ),
                )
            })?;
            lines.push(line.to_owned());
        }

        Ok(lines)
    }

    /// Scans executable G-code for the last coordinate-system word (G54..G59.3).
    /// Parenthesis comments and `;` line comments never trigger a match, and
    /// `G54.1 Pn` (which GRBL does not support) is deliberately ignored.
    pub(super) fn coordinate_system_select(command: &str) -> Option<CoordinateSystem> {
        let mut selected = None;
        let mut in_parenthesis_comment = false;

        for line in command.lines() {
            for (index, byte) in line.bytes().enumerate() {
                match byte {
                    b'(' => {
                        in_parenthesis_comment = true;
                        continue;
                    }
                    b')' => {
                        in_parenthesis_comment = false;
                        continue;
                    }
                    b';' if !in_parenthesis_comment => break,
                    b'G' | b'g' if !in_parenthesis_comment => {
                        let bytes = line.as_bytes();
                        let mut end = index + 1;
                        while end < bytes.len()
                            && (bytes[end].is_ascii_digit() || bytes[end] == b'.')
                        {
                            end += 1;
                        }

                        if end == index + 1 {
                            continue;
                        }

                        let code = line[index + 1..end].parse::<f64>().ok()?;
                        let system = match code {
                            54.0 => CoordinateSystem::G54,
                            55.0 => CoordinateSystem::G55,
                            56.0 => CoordinateSystem::G56,
                            57.0 => CoordinateSystem::G57,
                            58.0 => CoordinateSystem::G58,
                            59.0 => CoordinateSystem::G59,
                            59.1 => CoordinateSystem::G59_1,
                            59.2 => CoordinateSystem::G59_2,
                            59.3 => CoordinateSystem::G59_3,
                            _ => continue,
                        };
                        selected = Some(system);
                    }
                    _ => {}
                }
            }
        }

        selected
    }

    /// 1-based `G10 L20 P` parameter index for a coordinate system.
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

    /// Direction components must be -1/0/1; a real move needs a positive
    /// finite feed rate, while the all-zero (stop) request does not.
    pub(super) fn validate_continuous_xy_jog(
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        if ![-1, 0, 1].contains(&x_direction) || ![-1, 0, 1].contains(&y_direction) {
            return Err(ControllerError::new("bad_jog", "invalid continuous XY jog"));
        }

        // Feed rate is not needed for stopping when both components are zero.
        if x_direction == 0 && y_direction == 0 {
            return Ok(());
        }

        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }

        Ok(())
    }
}
