 //! Applies G-code coordinate-system selection and G10 work-offset updates.
use super::super::protocol::{api_error, WireError};
use super::super::state::Emulator;
use crate::cnc_api::*;

pub(super) enum GWordAction {
    Handled,
    Motion(i32),
    Dwell,
}

pub(super) fn handle_g_word(emulator: &mut Emulator, value: f64) -> Result<GWordAction, WireError> {
    if (value - 59.1).abs() < 1e-6 {
        emulator.select_coordinate_system_from_gcode(CoordinateSystem::G59_1);
        return Ok(GWordAction::Handled);
    }
    if (value - 59.2).abs() < 1e-6 {
        emulator.select_coordinate_system_from_gcode(CoordinateSystem::G59_2);
        return Ok(GWordAction::Handled);
    }
    if (value - 59.3).abs() < 1e-6 {
        emulator.select_coordinate_system_from_gcode(CoordinateSystem::G59_3);
        return Ok(GWordAction::Handled);
    }

    match value as i32 {
        0..=3 => Ok(GWordAction::Motion(value as i32)),
        4 => Ok(GWordAction::Dwell),
        17 | 21 => Ok(GWordAction::Handled),
        54 => {
            emulator.select_coordinate_system_from_gcode(CoordinateSystem::G54);
            Ok(GWordAction::Handled)
        }
        55 => {
            emulator.select_coordinate_system_from_gcode(CoordinateSystem::G55);
            Ok(GWordAction::Handled)
        }
        56 => {
            emulator.select_coordinate_system_from_gcode(CoordinateSystem::G56);
            Ok(GWordAction::Handled)
        }
        57 => {
            emulator.select_coordinate_system_from_gcode(CoordinateSystem::G57);
            Ok(GWordAction::Handled)
        }
        58 => {
            emulator.select_coordinate_system_from_gcode(CoordinateSystem::G58);
            Ok(GWordAction::Handled)
        }
        59 => {
            emulator.select_coordinate_system_from_gcode(CoordinateSystem::G59);
            Ok(GWordAction::Handled)
        }
        90 => {
            emulator.absolute_mode = true;
            Ok(GWordAction::Handled)
        }
        91 => {
            emulator.absolute_mode = false;
            Ok(GWordAction::Handled)
        }
        20 => Err(api_error(
            "unsupported_gcode",
            "G20 inch units are not supported; use G21",
        )),
        other => Err(api_error(
            "unsupported_gcode",
            format!("unsupported G-code G{other}"),
        )),
    }
}

pub(super) fn handle_g10_l2(
    emulator: &mut Emulator,
    l_word: Option<f64>,
    p_word: Option<f64>,
    x_word: Option<f64>,
    y_word: Option<f64>,
    z_word: Option<f64>,
    a_word: Option<f64>,
    b_word: Option<f64>,
) -> Result<(), WireError> {
    if l_word.map(|value| value as i32) != Some(2) {
        return Err(api_error("unsupported_gcode", "only G10 L2 is supported"));
    }

    let p = p_word.ok_or_else(|| api_error("bad_g10", "G10 L2 requires P1..P9"))? as i32;
    let system = match p {
        1 => CoordinateSystem::G54,
        2 => CoordinateSystem::G55,
        3 => CoordinateSystem::G56,
        4 => CoordinateSystem::G57,
        5 => CoordinateSystem::G58,
        6 => CoordinateSystem::G59,
        7 => CoordinateSystem::G59_1,
        8 => CoordinateSystem::G59_2,
        9 => CoordinateSystem::G59_3,
        _ => return Err(api_error("bad_g10", "G10 L2 P must be in range 1..9")),
    };

    let mut offset = emulator
        .status
        .coordinate_systems
        .iter()
        .find(|entry| entry.system == system)
        .map(|entry| entry.offset)
        .unwrap_or(Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            a: 0.0,
            b: 0.0,
        });

    if let Some(x) = x_word {
        offset.x = x;
    }
    if let Some(y) = y_word {
        offset.y = y;
    }
    if let Some(z) = z_word {
        offset.z = z;
    }
    if let Some(a) = a_word {
        offset.a = a;
    }
    if let Some(b) = b_word {
        offset.b = b;
    }

    if let Some(entry) = emulator
        .status
        .coordinate_systems
        .iter_mut()
        .find(|entry| entry.system == system)
    {
        entry.offset = offset;
    } else {
        emulator
            .status
            .coordinate_systems
            .push(CoordinateSystemOffset { system, offset });
    }

    if emulator.status.active_coordinate_system == system {
        emulator.status.work_offset = offset;
    }

    emulator.persist_coordinate_offsets_from_gcode()?;
    emulator.status.message = Some(format!("G10 L2 {} offset saved", system.label()));
    Ok(())
}
