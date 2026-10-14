use super::super::protocol::{api_error, WireError};
use super::super::state::Emulator;
use super::accessories;
use super::coordinates;
use super::motion;
use super::parser::parse_words;

pub(super) fn execute_gcode(emulator: &mut Emulator, line: &str) -> Result<(), WireError> {
    let words = parse_words(line);
    let mut motion_code = None;
    let mut dwell = false;
    let mut p_word = None;
    let mut x_word = None;
    let mut y_word = None;
    let mut z_word = None;
    let mut i_word = None;
    let mut j_word = None;
    let mut l_word = None;

    for &(letter, value) in &words {
        match letter {
            'G' => match coordinates::handle_g_word(emulator, value)? {
                coordinates::GWordAction::Motion(code) => motion_code = Some(code),
                coordinates::GWordAction::Dwell => dwell = true,
                coordinates::GWordAction::Handled => {}
            },
            'X' => x_word = Some(value),
            'Y' => y_word = Some(value),
            'Z' => z_word = Some(value),
            'I' => i_word = Some(value),
            'J' => j_word = Some(value),
            'P' => p_word = Some(value),
            'L' => l_word = Some(value),
            'F' if value.is_finite() && value > 0.0 => emulator.status.feed_rate = value,
            'F' => return Err(api_error("bad_feed", "F must be positive and finite")),
            'S' if value.is_finite() && value >= 0.0 => {
                if value > emulator.config.spindle.max_rpm {
                    return Err(api_error(
                        "spindle_limit",
                        format!(
                            "S{value:.0} exceeds maximum {:.0} RPM",
                            emulator.config.spindle.max_rpm
                        ),
                    ));
                }
                emulator.status.spindle.commanded_rpm = value;
            }
            'S' => {
                return Err(api_error(
                    "bad_spindle",
                    "S must be finite and non-negative",
                ))
            }
            'M' => accessories::handle_m_word(emulator, value)?,
            'N' | 'T' | 'H' => {}
            _ => {}
        }
    }

    if words
        .iter()
        .any(|(letter, value)| *letter == 'G' && (*value - 10.0).abs() < 1e-6)
    {
        coordinates::handle_g10_l2(emulator, l_word, p_word, x_word, y_word, z_word)?;
        return Ok(());
    }

    if dwell {
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

    if let Some(code) = motion_code {
        motion::execute_motion(emulator, code, x_word, y_word, z_word, i_word, j_word)?;
    } else if x_word.is_some() || y_word.is_some() || z_word.is_some() {
        return Err(api_error(
            "unsupported_gcode",
            "axis words require an explicit supported motion G0/G1/G2/G3",
        ));
    }

    emulator.status.message = Some(format!("Executing: {line}"));
    Ok(())
}
