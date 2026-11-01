//! Handles G-code M-words, tool selection, and simulated spindle state transitions.
use super::super::protocol::{api_error, WireError};
use super::super::state::Emulator;
use crate::cnc_api::*;

pub(super) fn handle_m_word(
    emulator: &mut Emulator,
    value: f64,
    q_word: Option<f64>,
) -> Result<(), WireError> {
    match value as i32 {
        3 => emulator.set_spindle(SpindleDirection::Clockwise, None),
        4 => emulator.set_spindle(SpindleDirection::CounterClockwise, None),
        5 => emulator.set_spindle(SpindleDirection::Stopped, Some(0.0)),
        7 => {
            emulator.status.mist_coolant_on = true;
            emulator.status.message = Some("Mist coolant ON (simulated)".into());
        }
        8 => {
            emulator.status.flood_coolant_on = true;
            emulator.status.message = Some("Flood coolant ON (simulated)".into());
        }
        9 => {
            emulator.status.mist_coolant_on = false;
            emulator.status.flood_coolant_on = false;
            emulator.status.message = Some("Coolant OFF".into());
        }
        30 => emulator.status.message = Some("Program end M30".into()),
        2 => emulator.status.message = Some("Program end M2".into()),
        0 => {
            emulator.status.interpreter_state = crate::cnc_api::InterpreterState::Paused;
            emulator.status.message = Some("Program paused for operator (M0)".into());
        }
        1 => {
            emulator.status.interpreter_state = crate::cnc_api::InterpreterState::Paused;
            emulator.status.message = Some("Optional program stop (M1)".into());
        }
        6 => change_tool(emulator)?,
        61 => select_current_tool(emulator, q_word)?,
        other => {
            return Err(api_error(
                "unsupported_gcode",
                format!("unsupported M-code M{other}"),
            ));
        }
    }
    Ok(())
}

/// Validates a T word and records its tool number for the following M6 block.
pub(super) fn select_tool(emulator: &mut Emulator, value: f64) -> Result<(), WireError> {
    if !value.is_finite() || value < 0.0 || value.fract().abs() > 1e-6 || value > u16::MAX as f64 {
        return Err(api_error(
            "bad_tool",
            "T must be an integer in the range 0..65535",
        ));
    }
    let tool = value as u16;
    emulator.selected_tool = Some(tool);
    emulator.status.selected_tool = Some(tool);
    emulator.status.message = Some(format!("Selected tool T{tool}"));
    Ok(())
}

/// Simulates a tool change by mounting the most recently selected T word.
pub(super) fn change_tool(emulator: &mut Emulator) -> Result<(), WireError> {
    let was_reading =
        emulator.status.interpreter_state == crate::cnc_api::InterpreterState::Reading;
    if was_reading {
        emulator.status.interpreter_state = crate::cnc_api::InterpreterState::Paused;
    }
    let tool = emulator
        .selected_tool
        .ok_or_else(|| api_error("bad_tool_change", "M6 requires a preceding T word"))?;
    emulator.current_tool = Some(tool);
    emulator.status.current_tool = Some(tool);
    if was_reading {
        emulator.status.interpreter_state = crate::cnc_api::InterpreterState::Reading;
    }
    emulator.status.message = Some(format!("Tool change complete: T{tool}"));
    Ok(())
}

/// Implements LinuxCNC-style M61 Qn, which selects the mounted tool without a changer.
pub(super) fn select_current_tool(
    emulator: &mut Emulator,
    q_word: Option<f64>,
) -> Result<(), WireError> {
    let value = q_word.ok_or_else(|| api_error("bad_tool", "M61 requires a Q tool number"))?;
    if !value.is_finite() || value < 0.0 || value.fract().abs() > 1e-6 || value > u16::MAX as f64 {
        return Err(api_error(
            "bad_tool",
            "M61 Q must be an integer in the range 0..65535",
        ));
    }
    let tool = value as u16;
    emulator.selected_tool = Some(tool);
    emulator.current_tool = Some(tool);
    emulator.status.selected_tool = Some(tool);
    emulator.status.current_tool = Some(tool);
    emulator.status.message = Some(format!("Current tool set to T{tool}"));
    Ok(())
}

/// Applies a spindle direction/speed request while preserving the configured ramp behavior.
pub(super) fn set_spindle(emulator: &mut Emulator, direction: SpindleDirection, rpm: Option<f64>) {
    let requested_rpm = rpm
        .unwrap_or(emulator.status.spindle.commanded_rpm)
        .clamp(0.0, emulator.config.spindle.max_rpm);

    if direction == SpindleDirection::Stopped || requested_rpm <= 0.0 {
        emulator.pending_spindle = None;
        emulator.status.spindle.commanded_rpm = 0.0;

        if emulator.status.spindle.actual_rpm <= 0.01 {
            emulator.status.spindle.actual_rpm = 0.0;
            emulator.status.spindle.direction = SpindleDirection::Stopped;
            emulator.status.spindle.enabled = false;
        } else {
            emulator.status.spindle.enabled = true;
        }
        return;
    }

    let current_direction = emulator.status.spindle.direction;
    let is_running = emulator.status.spindle.actual_rpm > 0.01;

    if is_running && current_direction != direction {
        emulator.pending_spindle = Some((direction, requested_rpm));
        emulator.status.spindle.commanded_rpm = 0.0;
        emulator.status.spindle.enabled = true;
    } else {
        emulator.pending_spindle = None;
        emulator.status.spindle.direction = direction;
        emulator.status.spindle.commanded_rpm = requested_rpm;
        emulator.status.spindle.enabled = true;
    }
}
