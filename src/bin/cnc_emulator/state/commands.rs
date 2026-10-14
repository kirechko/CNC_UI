use super::Emulator;
use super::super::dynamics::{ActiveMove, Motion};
use super::super::protocol::{api_error, WireError};
use crate::cnc_api::*;
use serde_json::Value;

pub(super) fn power_on(emulator: &mut Emulator) -> Result<Value, WireError> {
    emulator.status.task_state = TaskState::On;
    emulator.status.message = Some("Machine powered on".into());
    Ok(Value::Null)
}

pub(super) fn power_off(
    emulator: &mut Emulator,
    emergency_stop: bool,
) -> Result<Value, WireError> {
    emulator.dwell_remaining = 0.0;
    emulator.status.task_state = if emergency_stop {
        TaskState::Estop
    } else {
        TaskState::Off
    };
    emulator.status.interpreter_state = InterpreterState::Idle;
    emulator.status.spindle.enabled = false;
    emulator.status.mist_coolant_on = false;
    emulator.status.flood_coolant_on = false;
    emulator.status.spindle.commanded_rpm = 0.0;
    emulator.status.spindle.actual_rpm = 0.0;
    emulator.status.spindle.direction = SpindleDirection::Stopped;
    emulator.pending_spindle = None;
    emulator.active_move = None;
    emulator.continuous_jog = None;
    emulator.jog_velocity = 0.0;
    emulator.homing_queue.clear();
    emulator.homing_axis = None;
    emulator.active_move_is_homing = false;
    emulator.status.homing = false;
    if emergency_stop {
        emulator.status.homed = [false; 3];
    }
    emulator.status.message = Some(
        if emergency_stop {
            "Emergency stop"
        } else {
            "Machine powered off"
        }
        .into(),
    );
    Ok(Value::Null)
}

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

pub(super) fn jog_increment(
    emulator: &mut Emulator,
    args: Value,
) -> Result<Value, WireError> {
    emulator.ensure_motion_allowed()?;
    if emulator.active_move.is_some() {
        return Err(api_error(
            "motion_busy",
            "another movement is already in progress",
        ));
    }

    let request: JogRequest = serde_json::from_value(
        args.get("request").cloned().unwrap_or(Value::Null),
    )
    .map_err(|error| api_error("bad_request", error.to_string()))?;

    if ![-1, 1].contains(&request.direction)
        || !request.distance.is_finite()
        || request.distance < 0.0
    {
        return Err(api_error("bad_jog", "invalid jog direction or distance"));
    }

    let index = super::axis_index(request.axis);
    let feed = request.feed_rate.unwrap_or(emulator.status.feed_rate);
    let max_feed = emulator.config.axis(request.axis).max_velocity_mm_min;
    if !feed.is_finite() || feed <= 0.0 {
        return Err(api_error("bad_feed", "feed rate must be positive"));
    }

    let mut target = emulator.position_array();
    target[index] += request.distance * f64::from(request.direction);
    emulator.check_target(target)?;

    emulator.status.feed_rate = feed.min(max_feed);
    emulator.status.task_mode = TaskMode::Manual;
    emulator.status.message = Some(format!(
        "Jog {:?} {:+.3} at {:.0} mm/min",
        request.axis,
        request.distance * f64::from(request.direction),
        emulator.status.feed_rate
    ));

    let motion = Motion::new(
        emulator.position_array(),
        target,
        emulator.status.feed_rate,
        false,
        &emulator.config,
    )?;
    emulator.active_move = motion.map(ActiveMove::from_motion);
    Ok(Value::Null)
}

pub(super) fn jog_continuous(
    emulator: &mut Emulator,
    args: Value,
) -> Result<Value, WireError> {
    emulator.ensure_motion_allowed()?;
    let axis: Axis = serde_json::from_value(
        args.get("axis").cloned().unwrap_or(Value::Null),
    )
    .map_err(|error| api_error("bad_request", error.to_string()))?;
    let direction = args.get("direction").and_then(Value::as_i64).unwrap_or(0) as i8;
    let feed = args.get("feed_rate").and_then(Value::as_f64).unwrap_or(0.0);

    if direction == 0 {
        emulator.continuous_jog = None;
        emulator.jog_velocity = 0.0;
    } else if ![-1, 1].contains(&direction) || !feed.is_finite() || feed <= 0.0 {
        return Err(api_error(
            "bad_jog",
            "invalid continuous jog parameters",
        ));
    } else {
        let feed = feed.min(emulator.config.axis(axis).max_velocity_mm_min);
        emulator.continuous_jog = Some((axis, direction, feed));
        emulator.status.feed_rate = feed;
    }
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
