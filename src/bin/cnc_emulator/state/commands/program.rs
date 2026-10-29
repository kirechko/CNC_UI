//! MDI and program lifecycle commands (load, run, pause, resume, abort).
//!
//! Program execution itself is driven by the emulator tick loop; these
//! handlers only validate requests and switch the interpreter/task-mode
//! state machine, plus the forced Z retract performed on abort.

use super::super::Emulator;
use crate::cnc_api::*;
use crate::dynamics::{ActiveMove, Motion};
use crate::protocol::{api_error, WireError};
use serde_json::Value;

/// Execute one line of G-code immediately in MDI mode.
pub(super) fn mdi(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    emulator.ensure_motion_allowed()?;
    emulator.status.task_mode = TaskMode::Mdi;
    emulator.execute_gcode(args.get("command").and_then(Value::as_str).unwrap_or(""))?;
    Ok(Value::Null)
}

/// Load a program into memory, stripping comments and blank lines so the
/// tick loop executes only real blocks.
pub(super) fn load_program(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("program.ngc");
    let content = args.get("content").and_then(Value::as_str).unwrap_or("");

    emulator.program = content
        .lines()
        .map(|line| line.split(';').next().unwrap_or("").trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    emulator.status.loaded_program = Some(name.to_string());
    emulator.status.total_lines = emulator.program.len() as u32;
    emulator.status.current_line = 0;
    emulator.status.task_mode = crate::cnc_api::TaskMode::Auto;
    emulator.status.interpreter_state = crate::cnc_api::InterpreterState::Idle;
    emulator.status.message = Some(format!("Loaded {} blocks", emulator.program.len()));
    Ok(Value::Null)
}

/// Start (or restart) automatic program execution from the given line.
pub(super) fn program_run(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    emulator.ensure_motion_allowed()?;
    if emulator.stop_retract_active || emulator.active_move.is_some() {
        return Err(api_error(
            "motion_busy",
            "cannot start while stop retract is active",
        ));
    }
    let start = args.get("start_line").and_then(Value::as_u64).unwrap_or(0) as usize;
    if emulator.program.is_empty() || start > emulator.program.len() {
        return Err(api_error(
            "bad_program",
            "no program loaded or invalid start line",
        ));
    }
    emulator.status.current_line = start as u32;
    emulator.status.task_mode = TaskMode::Auto;
    emulator.status.interpreter_state = InterpreterState::Reading;
    emulator.status.message = Some("Program running".into());
    Ok(Value::Null)
}

/// Pause program execution while it is actively reading blocks.
pub(super) fn program_pause(emulator: &mut Emulator) -> Result<Value, WireError> {
    if emulator.status.interpreter_state == InterpreterState::Reading
        && !emulator.stop_retract_active
    {
        emulator.status.interpreter_state = InterpreterState::Paused;
        emulator.status.message = Some("Program paused".into());
    }
    Ok(Value::Null)
}

/// Resume a paused program by putting the interpreter back into reading.
pub(super) fn program_resume(emulator: &mut Emulator) -> Result<Value, WireError> {
    if emulator.status.interpreter_state == InterpreterState::Paused {
        emulator.status.interpreter_state = InterpreterState::Reading;
    }
    Ok(Value::Null)
}

/// Abort the program: flush all motion/jog/spindle/coolant state, then, if
/// a safe rapid Z height was recorded, retract Z there at rapid feed.
pub(super) fn program_abort(emulator: &mut Emulator) -> Result<Value, WireError> {
    emulator.dwell_remaining = 0.0;
    emulator.status.interpreter_state = InterpreterState::Idle;
    emulator.status.current_line = 0;
    emulator.active_move = None;
    emulator.continuous_jog = None;
    emulator.continuous_jog_xy = None;
    emulator.jog_move_active = false;
    emulator.jog_stop = None;
    emulator.active_move_is_rapid = false;
    emulator.jog_velocity = 0.0;
    emulator.status.jog_rate = 0.0;
    emulator.status.feed_rate = 0.0;
    emulator.status.motion_mode = None;

    // STOP forcibly turns the spindle off without performing a ramp-down.
    emulator.pending_spindle = None;
    emulator.status.spindle.commanded_rpm = 0.0;
    emulator.status.spindle.actual_rpm = 0.0;
    emulator.status.spindle.direction = SpindleDirection::Stopped;
    emulator.status.spindle.enabled = false;
    emulator.status.mist_coolant_on = false;
    emulator.status.flood_coolant_on = false;

    emulator.stop_retract_active = false;
    if let Some(safe_machine_z) = emulator.last_rapid_z {
        let mut target = emulator.position_array();
        target[2] = safe_machine_z;
        if emulator.check_target(target).is_ok()
            && (target[2] - emulator.status.position.z).abs() > 1e-6
        {
            let rapid_feed = emulator.config.axis_limits(crate::cnc_api::Axis::Z).2;
            match Motion::new(
                emulator.position_array(),
                target,
                rapid_feed,
                true,
                &emulator.config,
            ) {
                Ok(Some(motion)) => {
                    emulator.active_move = Some(ActiveMove::from_motion(motion));
                    emulator.stop_retract_active = true;
                    emulator.status.message = Some(format!(
                        "Program stopped; retracting Z to {:.3} mm",
                        safe_machine_z - emulator.status.work_offset.z
                    ));
                    return Ok(Value::Null);
                }
                Ok(None) => {}
                Err(error) => {
                    emulator.status.message = Some(format!(
                        "Program stopped; retract failed: {}",
                        error.message
                    ));
                    return Ok(Value::Null);
                }
            }
        }
    }

    emulator.status.message = Some("Program stopped; no retract required".into());
    Ok(Value::Null)
}
