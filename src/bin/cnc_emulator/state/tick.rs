use super::super::dynamics::{ActiveMove, Motion};
use super::Emulator;
use crate::cnc_api::*;

pub(super) fn tick(emulator: &mut Emulator, dt: f64) {
    if emulator.dwell_remaining > 0.0
        && emulator.status.interpreter_state != InterpreterState::Paused
    {
        emulator.dwell_remaining = (emulator.dwell_remaining - dt).max(0.0);
        if emulator.dwell_remaining > 0.0 {
            return;
        }
        emulator.status.message = Some("Dwell complete".into());
        return;
    }
    let target_rpm = emulator.status.spindle.commanded_rpm;
    let actual_rpm = emulator.status.spindle.actual_rpm;
    let max_delta = emulator.config.spindle.acceleration_rpm_per_sec * dt;
    let next_rpm = if actual_rpm < target_rpm {
        (actual_rpm + max_delta).min(target_rpm)
    } else {
        (actual_rpm - max_delta).max(target_rpm)
    };
    emulator.status.spindle.actual_rpm = next_rpm;
    if target_rpm <= 0.0 && next_rpm <= 0.01 {
        emulator.status.spindle.actual_rpm = 0.0;

        if let Some((direction, rpm)) = emulator.pending_spindle.take() {
            emulator.status.spindle.direction = direction;
            emulator.status.spindle.commanded_rpm = rpm;
            emulator.status.spindle.enabled = rpm > 0.0;
        } else {
            emulator.status.spindle.direction = SpindleDirection::Stopped;
            emulator.status.spindle.enabled = false;
        }
    } else {
        emulator.status.spindle.enabled = next_rpm > 0.01 || target_rpm > 0.0;
    }

    let movement_may_advance =
        emulator.stop_retract_active
            || emulator.status.interpreter_state != InterpreterState::Paused;

    if movement_may_advance {
        if let Some(mut movement) = emulator.active_move.take() {
            let (position, done) = movement.tick(dt);
            emulator.set_position(position);

            if done {
                if emulator.stop_retract_active {
                    emulator.stop_retract_active = false;
                    emulator.status.message = Some("Stop retract complete".into());
                }
                if emulator.active_move_is_homing {
                    if let Some(axis) = emulator.homing_axis.take() {
                        emulator.status.homed[super::axis_index(axis)] = true;
                    }
                    emulator.active_move_is_homing = false;
                }
            } else {
                emulator.active_move = Some(movement);
            }
        }
    }

    if emulator.active_move.is_none() && emulator.status.homing {
        if let Some(axis) = emulator.homing_queue.first().copied() {
            emulator.homing_queue.remove(0);
            let mut target = emulator.position_array();
            let index = super::axis_index(axis);
            target[index] = emulator.config.axis(axis).min_mm;
            emulator.homing_axis = Some(axis);

            match Motion::new(
                emulator.position_array(),
                target,
                emulator.config.axis(axis).max_velocity_mm_min,
                true,
                &emulator.config,
            ) {
                Ok(Some(motion)) => {
                    emulator.active_move = Some(ActiveMove::from_motion(motion));
                    emulator.active_move_is_homing = true;
                }
                Ok(None) => {
                    emulator.status.homed[index] = true;
                    emulator.homing_axis = None;
                }
                Err(error) => {
                    emulator.status.homing = false;
                    emulator.status.task_state = TaskState::Estop;
                    emulator.status.message = Some(format!("Homing error: {}", error.message));
                    emulator.homing_queue.clear();
                    emulator.homing_axis = None;
                    emulator.active_move_is_homing = false;
                    return;
                }
            }
        } else {
            emulator.status.homing = false;
            emulator.status.message = Some("Homing complete".into());
        }
    }

    if let Some((axis, direction, feed)) = emulator.continuous_jog {
        let limits = emulator.config.axis(axis);
        let max_v = (feed / 60.0).min(limits.max_velocity_mm_min / 60.0);
        emulator.jog_velocity =
            (emulator.jog_velocity + limits.acceleration_mm_s2 * dt).min(max_v);
        let delta = f64::from(direction) * emulator.jog_velocity * dt;
        let mut position = emulator.position_array();
        let index = super::axis_index(axis);
        position[index] = (position[index] + delta).clamp(limits.min_mm, limits.max_mm);
        emulator.set_position(position);
    } else {
        emulator.jog_velocity = 0.0;
    }

    if emulator.status.interpreter_state != InterpreterState::Reading
        || emulator.active_move.is_some()
        || emulator.status.homing
    {
        return;
    }

    let index = emulator.status.current_line as usize;
    if index >= emulator.program.len() {
        emulator.status.interpreter_state = InterpreterState::Idle;
        emulator.status.message = Some("Program finished".into());
        return;
    }

    let line = emulator.program[index].clone();
    match emulator.execute_gcode(&line) {
        Ok(()) => {
            emulator.status.current_line += 1;
            if emulator.status.current_line as usize >= emulator.program.len()
                && emulator.active_move.is_none()
            {
                emulator.status.interpreter_state = InterpreterState::Idle;
                emulator.status.message = Some("Program finished".into());
            }
        }
        Err(error) => {
            emulator.status.interpreter_state = InterpreterState::Idle;
            emulator.status.message = Some(format!("{}: {}", error.code, error.message));
        }
    }
}
