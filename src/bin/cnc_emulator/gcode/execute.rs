//! Dispatches parsed G-code words to the coordinate, motion, and accessory handlers.
use super::super::protocol::{api_error, WireError};
use super::super::state::Emulator;
use super::accessories;
use super::coordinates;
use super::motion;
use super::parser::parse_words;
use crate::cnc_api::Axis;

/// Parses and executes one G-code line against the emulator state, validating its words before motion.
pub(super) fn execute_gcode(emulator: &mut Emulator, line: &str) -> Result<(), WireError> {
    let words = parse_words(line);
    let mut motion_code = None;
    let mut dwell = false;
    let mut tool_change = false;
    let mut set_current_tool = false;
    let mut q_word = None;
    let mut p_word = None;
    let mut x_word = None;
    let mut y_word = None;
    let mut z_word = None;
    let mut a_word = None;
    let mut b_word = None;
    let mut i_word = None;
    let mut j_word = None;
    let mut l_word = None;

    for &(letter, value) in &words {
        match letter {
            'G' if (value - 10.0).abs() < 1e-6 => {}
            'G' if (value - 38.2).abs() < 1e-6 => motion_code = Some(38),
            'G' => match coordinates::handle_g_word(emulator, value)? {
                coordinates::GWordAction::Motion(code) => motion_code = Some(code),
                coordinates::GWordAction::Dwell => dwell = true,
                coordinates::GWordAction::Handled => {}
            },
            'X' => x_word = Some(value),
            'Y' => y_word = Some(value),
            'Z' => z_word = Some(value),
            'A' => a_word = Some(value),
            'B' => b_word = Some(value),
            'C' => {}
            'I' => i_word = Some(value),
            'J' => j_word = Some(value),
            'P' => p_word = Some(value),
            'Q' => q_word = Some(value),
            'L' => l_word = Some(value),
            'T' => accessories::select_tool(emulator, value)?,
            'F' if value.is_finite() && value > 0.0 => emulator.program_feed_mm_min = value,
            'F' => return Err(api_error("bad_feed", "F must be positive and finite")),
            'S' if value.is_finite() && value >= 0.0 => {
                let maximum = emulator
                    .config
                    .spindle
                    .laser
                    .map(|laser| laser.max_power)
                    .unwrap_or(emulator.config.spindle.max_rpm);
                if value > maximum {
                    let unit =
                        if emulator.config.spindle.mode == crate::cnc_api::SpindleMode::Standard {
                            "RPM"
                        } else {
                            "laser power"
                        };
                    return Err(api_error(
                        "spindle_limit",
                        format!("S{value:.0} exceeds configured maximum {maximum:.0} {unit}"),
                    ));
                }
                // S is modal; in laser mode it updates PWM telemetry even while M5 keeps emission disabled.
                emulator.status.spindle.commanded_rpm = value;
                if emulator.status.spindle.mode != crate::cnc_api::SpindleMode::Standard {
                    emulator.status.spindle.laser_power = Some(value);
                }
            }
            'S' => {
                return Err(api_error(
                    "bad_spindle",
                    "S must be finite and non-negative",
                ))
            }
            'M' if (value - 6.0).abs() < 1e-6 => tool_change = true,
            'M' if (value - 61.0).abs() < 1e-6 => set_current_tool = true,
            'M' if (value - 0.0).abs() < 1e-6 || (value - 1.0).abs() < 1e-6 => {
                accessories::handle_m_word(emulator, value, q_word)?;
            }
            'M' => accessories::handle_m_word(emulator, value, q_word)?,
            'N' | 'H' => {}
            _ => {}
        }
    }

    if tool_change {
        accessories::change_tool(emulator)?;
    }
    if set_current_tool {
        accessories::select_current_tool(emulator, q_word)?;
    }

    if words
        .iter()
        .any(|(letter, value)| *letter == 'G' && (*value - 10.0).abs() < 1e-6)
    {
        coordinates::handle_g10_l2(
            emulator,
            coordinates::G10Words {
                l: l_word,
                p: p_word,
                x: x_word,
                y: y_word,
                z: z_word,
                a: a_word,
                b: b_word,
            },
        )?;
        return Ok(());
    }

    if dwell {
        // A zero-power pause is safer at the motion boundary; configured laser
        // profiles may intentionally preserve output only when auto-shutoff is disabled.
        if emulator
            .config
            .spindle
            .laser
            .is_some_and(|laser| laser.auto_shutoff_on_pause)
        {
            emulator.status.spindle.enabled = false;
            emulator.status.spindle.commanded_rpm = 0.0;
            emulator.status.spindle.actual_rpm = 0.0;
            emulator.status.spindle.direction = crate::cnc_api::SpindleDirection::Stopped;
            emulator.status.spindle.laser_power = Some(0.0);
        }
        let seconds = p_word
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| {
                api_error(
                    "bad_dwell",
                    "G4 requires a finite, non-negative P value in seconds",
                )
            })?;

        emulator.dwell_remaining = seconds;
        emulator.status.message = Some(format!("Dwell: {:.3} s", seconds));
        return Ok(());
    }

    if motion_code == Some(38) {
        // A G38.2 line is a stateful probe cycle; never run it with latent laser output enabled.
        let requested = [x_word, y_word, z_word, a_word, b_word];
        let (index, distance) = requested
            .iter()
            .enumerate()
            .find_map(|(index, value)| value.map(|distance| (index, distance)))
            .ok_or_else(|| api_error("bad_probe", "G38.2 requires one axis target"))?;
        if requested.iter().filter(|value| value.is_some()).count() != 1 {
            return Err(api_error(
                "bad_probe",
                "G38.2 supports one axis per probe block",
            ));
        }
        let start = emulator.position_array()[index];
        if emulator.status.spindle.mode != crate::cnc_api::SpindleMode::Standard
            && emulator
                .config
                .spindle
                .laser
                .is_some_and(|laser| laser.auto_shutoff_on_pause)
        {
            emulator.status.spindle.enabled = false;
            emulator.status.spindle.commanded_rpm = 0.0;
            emulator.status.spindle.actual_rpm = 0.0;
            emulator.status.spindle.direction = crate::cnc_api::SpindleDirection::Stopped;
            emulator.status.spindle.laser_power = Some(0.0);
        }
        emulator.status.probe_triggered = false;
        emulator.status.probe_position = None;
        let target = if emulator.absolute_mode {
            distance
                + match index {
                    0 => emulator.status.work_offset.x,
                    1 => emulator.status.work_offset.y,
                    2 => emulator.status.work_offset.z,
                    3 => emulator.status.work_offset.a,
                    _ => emulator.status.work_offset.b,
                }
        } else {
            start + distance
        };
        let axis = match index {
            0 => Axis::X,
            1 => Axis::Y,
            2 => Axis::Z,
            3 => Axis::A,
            _ => Axis::B,
        };
        if !emulator.config.configured_axes().contains(&axis) {
            return Err(api_error(
                "probe_axis",
                format!("axis {axis:?} is not configured for probing"),
            ));
        }
        let (axis_min, axis_max, _, _) = emulator.config.axis_limits(axis);
        if target < axis_min || target > axis_max {
            return Err(api_error(
                "probe_no_contact",
                "G38.2 probe reached its travel limit without contact",
            ));
        }
        let contact = emulator
            .probe_contact(index, start, target)
            .ok_or_else(|| api_error("probe_no_contact", "G38.2 probe did not touch the stock"))?;
        emulator.status.probe_triggered = true;
        let mut contact_position = emulator.status.position;
        match index {
            0 => contact_position.x = contact,
            1 => contact_position.y = contact,
            2 => contact_position.z = contact,
            _ => {}
        }
        emulator.status.probe_position = Some(contact_position);
        motion::execute_motion(
            emulator,
            motion::MotionWords {
                code: 1,
                x: (index == 0).then_some(if emulator.absolute_mode {
                    contact - emulator.status.work_offset.x
                } else {
                    contact - start
                }),
                y: (index == 1).then_some(if emulator.absolute_mode {
                    contact - emulator.status.work_offset.y
                } else {
                    contact - start
                }),
                z: (index == 2).then_some(if emulator.absolute_mode {
                    contact - emulator.status.work_offset.z
                } else {
                    contact - start
                }),
                a: (index == 3).then_some(if emulator.absolute_mode {
                    contact - emulator.status.work_offset.a
                } else {
                    contact - start
                }),
                b: (index == 4).then_some(if emulator.absolute_mode {
                    contact - emulator.status.work_offset.b
                } else {
                    contact - start
                }),
                i: None,
                j: None,
            },
        )?;
        emulator.status.message = Some(format!("Probe contact at {:.3}", contact));
        return Ok(());
    } else if let Some(code) = motion_code {
        motion::execute_motion(
            emulator,
            motion::MotionWords {
                code,
                x: x_word,
                y: y_word,
                z: z_word,
                a: a_word,
                b: b_word,
                i: i_word,
                j: j_word,
            },
        )?;
    } else if x_word.is_some()
        || y_word.is_some()
        || z_word.is_some()
        || a_word.is_some()
        || b_word.is_some()
    {
        return Err(api_error(
            "unsupported_gcode",
            "axis words require an explicit supported motion G0/G1/G2/G3",
        ));
    }

    let program_stop = words.iter().any(|(letter, value)| {
        *letter == 'M' && ((*value - 0.0).abs() < 1e-6 || (*value - 1.0).abs() < 1e-6)
    });
    if !program_stop {
        emulator.status.message = Some(format!("Executing: {line}"));
    }
    Ok(())
}
