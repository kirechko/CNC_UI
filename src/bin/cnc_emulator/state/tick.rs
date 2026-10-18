 //! Advances simulated spindle, motion, homing, jogging, and program execution.
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

    let movement_may_advance = emulator.stop_retract_active
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
            let (minimum, _, velocity, _) = emulator.config.axis_limits(axis);
            target[index] = minimum;
            emulator.homing_axis = Some(axis);

            match Motion::new(
                emulator.position_array(),
                target,
                velocity,
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

    if let Some((x_direction, y_direction, feed)) = emulator.continuous_jog_xy {
        let norm = f64::from(x_direction * x_direction + y_direction * y_direction).sqrt();
        let x_component = f64::from(x_direction) / norm;
        let y_component = f64::from(y_direction) / norm;
        let (_, _, x_velocity, x_acceleration) = emulator.config.axis_limits(Axis::X);
        let (_, _, y_velocity, y_acceleration) = emulator.config.axis_limits(Axis::Y);
        let max_velocity = (feed / 60.0)
            .min(if x_component.abs() > 1e-9 {
                x_velocity / 60.0 / x_component.abs()
            } else {
                f64::INFINITY
            })
            .min(if y_component.abs() > 1e-9 {
                y_velocity / 60.0 / y_component.abs()
            } else {
                f64::INFINITY
            });
        let acceleration = (if x_component.abs() > 1e-9 {
            x_acceleration / x_component.abs()
        } else {
            f64::INFINITY
        })
        .min(if y_component.abs() > 1e-9 {
            y_acceleration / y_component.abs()
        } else {
            f64::INFINITY
        });

        emulator.jog_velocity = (emulator.jog_velocity + acceleration * dt).min(max_velocity);
        let mut position = emulator.position_array();
        let (x_min, x_max, _, _) = emulator.config.axis_limits(Axis::X);
        let (y_min, y_max, _, _) = emulator.config.axis_limits(Axis::Y);
        position[0] = (position[0] + x_component * emulator.jog_velocity * dt)
            .clamp(x_min, x_max);
        position[1] = (position[1] + y_component * emulator.jog_velocity * dt)
            .clamp(y_min, y_max);
        emulator.set_position(position);
    } else if let Some((axis, direction, feed)) = emulator.continuous_jog {
        let (minimum, maximum, max_velocity, acceleration) = emulator.config.axis_limits(axis);
        let max_v = (feed / 60.0).min(max_velocity / 60.0);
        emulator.jog_velocity = (emulator.jog_velocity + acceleration * dt).min(max_v);
        let delta = f64::from(direction) * emulator.jog_velocity * dt;
        let mut position = emulator.position_array();
        let index = super::axis_index(axis);
        position[index] = (position[index] + delta).clamp(minimum, maximum);
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
