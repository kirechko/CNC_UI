//! Power-state, alarm, and spindle commands for the emulator dispatcher.
//!
//! These functions translate wire-level requests into mutations of the
//! shared [`Emulator`] status struct. They are invoked only through the
//! dispatcher in the parent `commands` module.

use super::super::Emulator;
use crate::cnc_api::*;
use crate::protocol::{api_error, WireError};
use serde_json::Value;

/// Power the virtual machine on: accept commands and clear any alarm gate.
pub(super) fn power_on(emulator: &mut Emulator) -> Result<Value, WireError> {
    emulator.status.task_state = TaskState::On;
    emulator.status.message = Some("Machine powered on".into());
    Ok(Value::Null)
}

/// Power the machine off, or trigger an emergency stop when
/// `emergency_stop` is set. Both paths tear down every in-flight motion,
/// spindle, and coolant state so no residual movement can survive.
pub(super) fn power_off(emulator: &mut Emulator, emergency_stop: bool) -> Result<Value, WireError> {
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

/// Apply a spindle command after validating the machine is on, the RPM is
/// finite/non-negative, and it does not exceed the configured maximum.
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
    if !command.rpm.is_finite() || command.rpm < 0.0 {
        return Err(api_error(
            "bad_spindle",
            "RPM must be finite and non-negative",
        ));
    }
    if command.rpm > emulator.config.spindle.max_rpm {
        return Err(api_error(
            "spindle_limit",
            format!(
                "requested {:.0} RPM exceeds configured maximum {:.0} RPM",
                command.rpm, emulator.config.spindle.max_rpm
            ),
        ));
    }
    emulator.set_spindle(command.direction, Some(command.rpm));
    Ok(Value::Null)
}
