//! Power-state, alarm, and spindle commands for the emulator dispatcher.
//!
//! These functions translate wire-level requests into mutations of the
//! shared [`Emulator`] status struct. They are invoked only through the
//! dispatcher in the parent `commands` module.

use super::super::super::protocol::{api_error, WireError};
use super::super::Emulator;
use crate::cnc_api::*;
use serde_json::Value;

/// Power the virtual machine on: accept commands and clear any alarm gate.
/// Enables the virtual machine and clears the soft power-off state.
pub(super) fn power_on(emulator: &mut Emulator) -> Result<Value, WireError> {
    emulator.status.task_state = TaskState::On;
    emulator.status.message = Some("Machine powered on".into());
    Ok(Value::Null)
}

/// Power the machine off, or trigger an emergency stop when
/// `emergency_stop` is set. Both paths tear down all motion, spindle/laser
/// output, and coolant state so no residual movement or emission can survive.
pub(super) fn power_off(emulator: &mut Emulator, emergency_stop: bool) -> Result<Value, WireError> {
    emulator.dwell_remaining = 0.0;
    emulator.status.task_state = if emergency_stop {
        TaskState::Estop
    } else {
        TaskState::Off
    };
    emulator.status.interpreter_state = InterpreterState::Idle;
    emulator.status.spindle.enabled = false;
    emulator.status.spindle.commanded_rpm = 0.0;
    emulator.status.spindle.actual_rpm = 0.0;
    emulator.status.spindle.laser_power =
        (emulator.status.spindle.mode != SpindleMode::Standard).then_some(0.0);
    emulator.status.mist_coolant_on = false;
    emulator.status.flood_coolant_on = false;
    emulator.status.spindle.direction = SpindleDirection::Stopped;
    emulator.pending_spindle = None;
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
    emulator.homing_queue.clear();
    emulator.homing_axis = None;
    emulator.active_move_is_homing = false;
    emulator.status.homing = false;
    if emergency_stop {
        emulator.status.homed = [false; 6];
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

/// Reset the alarm latch. An emergency stop can only move to the
/// `EstopReset` sub-state; otherwise the message is simply cleared.
pub(super) fn clear_error(emulator: &mut Emulator) -> Result<Value, WireError> {
    if emulator.status.task_state == TaskState::Estop {
        emulator.status.task_state = TaskState::EstopReset;
        emulator.status.message = Some("Alarm reset".into());
    } else {
        emulator.status.message = None;
    }
    Ok(Value::Null)
}

/// Selects constant/dynamic laser output only when the emulator has a laser profile.
pub(super) fn set_laser_mode(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    // Mode selection affects subsequent M3/M4 interpretation but never enables emission itself.
    let mode = args
        .get("mode")
        .and_then(Value::as_str)
        .ok_or_else(|| api_error("bad_laser_mode", "laser mode must be constant or dynamic"))?;
    let requested = match mode {
        "constant" => SpindleMode::LaserConstant,
        "dynamic" => SpindleMode::LaserDynamic,
        _ => {
            return Err(api_error(
                "bad_laser_mode",
                "laser mode must be constant or dynamic",
            ))
        }
    };
    let profile = emulator
        .config
        .spindle
        .laser
        .ok_or_else(|| api_error("laser_unavailable", "this machine has no laser profile"))?;
    if requested == SpindleMode::LaserDynamic && !profile.dynamic_power_supported {
        return Err(api_error(
            "laser_mode_unsupported",
            "dynamic laser mode is unavailable",
        ));
    }
    emulator.status.spindle.mode = requested;
    Ok(Value::Null)
}

/// Applies a spindle or laser output command after checking readiness and native S limits.
pub(super) fn set_spindle(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    if emulator.status.task_state != TaskState::On {
        return Err(api_error(
            "machine_not_ready",
            "spindle command rejected while machine is in Alarm or powered off",
        ));
    }

    let command: SpindleCommand =
        serde_json::from_value(args.get("command").cloned().unwrap_or(Value::Null))
            .map_err(|error| api_error("bad_request", error.to_string()))?;
    // Output values share the legacy `rpm` field in SpindleCommand; interpret the limit by mode.
    if !command.rpm.is_finite() || command.rpm < 0.0 {
        return Err(api_error(
            "bad_spindle",
            "spindle speed or laser power must be finite and non-negative",
        ));
    }
    if emulator.status.spindle.mode != SpindleMode::Standard
        && command.direction == SpindleDirection::CounterClockwise
    {
        return Err(api_error(
            "laser_direction",
            "laser output has no reverse direction",
        ));
    }
    let maximum = emulator
        .config
        .spindle
        .laser
        .map(|laser| laser.max_power)
        .unwrap_or(emulator.config.spindle.max_rpm);
    if command.rpm > maximum {
        return Err(api_error(
            "spindle_limit",
            format!(
                "requested {:.0} exceeds configured maximum {:.0}",
                command.rpm, maximum
            ),
        ));
    }
    emulator.set_spindle(command.direction, Some(command.rpm));
    Ok(Value::Null)
}
