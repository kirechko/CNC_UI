//! Coordinate-system, work-zero, and homing commands for the dispatcher.
//!
//! Work-zero selection/delegation lives in the sibling `coordinates` module
//! of `state`; these handlers only parse the wire arguments and forward them
//! to the matching `Emulator` methods.

use crate::protocol::{api_error, WireError};
use super::super::{axis_index, Emulator};
use crate::cnc_api::*;
use serde_json::Value;

/// Switch the active coordinate system (G54..G59.3).
pub(super) fn select_coordinate_system(
    emulator: &mut Emulator,
    args: Value,
) -> Result<Value, WireError> {
    let system: crate::cnc_api::CoordinateSystem =
        serde_json::from_value(args.get("system").cloned().unwrap_or(Value::Null))
            .map_err(|error| api_error("bad_request", error.to_string()))?;

    emulator.select_coordinate_system(system);
    Ok(Value::Null)
}

/// Set the work zero of every configured axis in the chosen system to the
/// current machine position.
pub(super) fn set_work_zero(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    let system: crate::cnc_api::CoordinateSystem =
        serde_json::from_value(args.get("system").cloned().unwrap_or(Value::Null))
            .map_err(|error| api_error("bad_request", error.to_string()))?;

    emulator.set_selected_work_zero(system)?;
    Ok(Value::Null)
}

/// Set the work zero of one axis in the active system to the current
/// machine position.
pub(super) fn set_axis_work_zero(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    let axis: Axis = serde_json::from_value(args.get("axis").cloned().unwrap_or(Value::Null))
        .map_err(|error| api_error("bad_request", error.to_string()))?;

    emulator.set_axis_work_zero(axis)?;
    Ok(Value::Null)
}

/// Queue a homing cycle for one axis or for all configured axes. Homing is
/// rejected while any motion, jog, or program reading is active.
pub(super) fn home(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    emulator.ensure_motion_allowed()?;
    if emulator.active_move.is_some()
        || emulator.continuous_jog.is_some()
        || emulator.status.interpreter_state == crate::cnc_api::InterpreterState::Reading
        || emulator.status.homing
    {
        return Err(api_error(
            "motion_busy",
            "cannot home while motion or program is active",
        ));
    }

    let requested: Option<crate::cnc_api::Axis> =
        serde_json::from_value(args.get("axis").cloned().unwrap_or(Value::Null))
            .map_err(|error| api_error("bad_request", error.to_string()))?;
    if requested == Some(Axis::C) {
        return Err(api_error(
            "unsupported_axis",
            "the emulator does not configure a C axis",
        ));
    }

    emulator.homing_queue = match requested {
        Some(axis) => vec![axis],
        None => emulator.config.configured_axes(),
    };
    for axis in &emulator.homing_queue {
        emulator.status.homed[axis_index(*axis)] = false;
    }

    emulator.status.homing = true;
    emulator.status.motion_mode = Some("Homing".into());
    emulator.status.task_mode = crate::cnc_api::TaskMode::Manual;
    emulator.status.message = Some("Homing started".into());
    Ok(Value::Null)
}
