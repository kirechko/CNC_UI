use super::Emulator;
use super::super::protocol::{api_error, WireError};
use crate::cnc_api::TaskState;
use serde_json::Value;

pub(super) fn select_coordinate_system(
    emulator: &mut Emulator,
    args: Value,
) -> Result<Value, WireError> {
    let system: crate::cnc_api::CoordinateSystem = serde_json::from_value(
        args.get("system").cloned().unwrap_or(Value::Null),
    )
    .map_err(|error| api_error("bad_request", error.to_string()))?;

    emulator.select_coordinate_system(system);
    Ok(Value::Null)
}

pub(super) fn set_work_zero(
    emulator: &mut Emulator,
    args: Value,
) -> Result<Value, WireError> {
    let system: crate::cnc_api::CoordinateSystem = serde_json::from_value(
        args.get("system").cloned().unwrap_or(Value::Null),
    )
    .map_err(|error| api_error("bad_request", error.to_string()))?;

    emulator.set_selected_work_zero(system)?;
    Ok(Value::Null)
}

pub(super) fn load_program(
    emulator: &mut Emulator,
    args: Value,
) -> Result<Value, WireError> {
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

pub(super) fn home(
    emulator: &mut Emulator,
    args: Value,
) -> Result<Value, WireError> {
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

    let requested: Option<crate::cnc_api::Axis> = serde_json::from_value(
        args.get("axis").cloned().unwrap_or(Value::Null),
    )
    .map_err(|error| api_error("bad_request", error.to_string()))?;

    emulator.homing_queue = match requested {
        Some(axis) => vec![axis],
        None => vec![
            crate::cnc_api::Axis::X,
            crate::cnc_api::Axis::Y,
            crate::cnc_api::Axis::Z,
        ],
    };
    for axis in &emulator.homing_queue {
        emulator.status.homed[super::axis_index(*axis)] = false;
    }

    emulator.status.homing = true;
    emulator.status.task_mode = crate::cnc_api::TaskMode::Manual;
    emulator.status.message = Some("Homing started".into());
    Ok(Value::Null)
}

impl Emulator {
    pub(super) fn ensure_motion_allowed(&self) -> Result<(), WireError> {
        if self.status.task_state != TaskState::On {
            return Err(api_error("machine_off", "virtual machine is not powered on"));
        }
        Ok(())
    }
}
