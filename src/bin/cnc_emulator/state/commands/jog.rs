//! Jog and motion-stop commands for the emulator dispatcher.
//!
//! Incremental jogs start a single finite move; continuous jogs register a
//! velocity target that the tick loop integrates until cancellation. The
//! private [`begin_jog_deceleration`] helper converts an active continuous
//! jog into a deceleration ramp so a stop request does not jerk the axis.

use super::super::{axis_index, Emulator};
use crate::cnc_api::*;
use crate::dynamics::{ActiveMove, Motion};
use crate::protocol::{api_error, WireError};
use serde_json::Value;

/// Perform one fixed-distance jog along a single axis.
pub(super) fn jog_increment(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    emulator.ensure_motion_allowed()?;
    if emulator.active_move.is_some() {
        return Err(api_error(
            "motion_busy",
            "another movement is already in progress",
        ));
    }

    let request: JogRequest =
        serde_json::from_value(args.get("request").cloned().unwrap_or(Value::Null))
            .map_err(|error| api_error("bad_request", error.to_string()))?;

    if request.axis == Axis::C {
        return Err(api_error(
            "unsupported_axis",
            "the emulator does not configure a C axis",
        ));
    }
    if ![-1, 1].contains(&request.direction)
        || !request.distance.is_finite()
        || request.distance < 0.0
    {
        return Err(api_error("bad_jog", "invalid jog direction or distance"));
    }

    let index = axis_index(request.axis);
    let feed = request.feed_rate.unwrap_or(emulator.status.feed_rate);
    let max_feed = emulator.config.axis_limits(request.axis).2;
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
    emulator.jog_move_active = emulator.active_move.is_some();
    emulator.active_move_is_rapid = false;
    if emulator.jog_move_active {
        emulator.status.motion_mode = Some("Jog".into());
    }
    Ok(Value::Null)
}

/// Start the smooth deceleration ramp for whichever continuous jog (single
/// axis or XY) is currently registered, or zero the velocity immediately if
/// no ramp can be built.
fn begin_jog_deceleration(emulator: &mut Emulator) {
    let motion = if let Some((axis, direction, _)) = emulator.continuous_jog.take() {
        let mut vector = [0.0; 5];
        vector[axis_index(axis)] = f64::from(direction);
        Some((vector, emulator.config.axis_limits(axis).3))
    } else if let Some((x_direction, y_direction, _)) = emulator.continuous_jog_xy.take() {
        let norm = f64::from(x_direction * x_direction + y_direction * y_direction).sqrt();
        let x_component = f64::from(x_direction) / norm;
        let y_component = f64::from(y_direction) / norm;
        let x_acceleration = emulator.config.axis_limits(Axis::X).3;
        let y_acceleration = emulator.config.axis_limits(Axis::Y).3;
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
        Some(([x_component, y_component, 0.0, 0.0, 0.0], acceleration))
    } else {
        None
    };

    if let Some((direction, acceleration)) = motion {
        if emulator.jog_velocity > 0.0 && acceleration.is_finite() {
            emulator.jog_stop = Some((direction, acceleration));
        } else {
            emulator.jog_velocity = 0.0;
            emulator.status.jog_rate = 0.0;
            emulator.jog_stop = None;
        }
    }
}

/// Cancel continuous jogging instantly (hard stop of the jog state).
pub(super) fn jog_cancel(emulator: &mut Emulator) -> Result<Value, WireError> {
    emulator.continuous_jog = None;
    emulator.continuous_jog_xy = None;
    emulator.jog_stop = None;
    if emulator.jog_move_active {
        emulator.active_move = None;
        emulator.jog_move_active = false;
    }
    emulator.jog_velocity = 0.0;
    emulator.status.jog_rate = 0.0;
    emulator.status.motion_mode = None;
    emulator.status.message = Some("Jog stopped".into());
    Ok(Value::Null)
}

/// Hard-stop every kind of motion (program move, jog, homing) and pause the
/// interpreter if a program was executing.
pub(super) fn stop_motion(emulator: &mut Emulator) -> Result<Value, WireError> {
    emulator.active_move = None;
    emulator.continuous_jog = None;
    emulator.continuous_jog_xy = None;
    emulator.jog_stop = None;
    emulator.jog_move_active = false;
    emulator.active_move_is_rapid = false;
    emulator.active_move_is_homing = false;
    emulator.homing_queue.clear();
    emulator.homing_axis = None;
    emulator.status.homing = false;
    emulator.jog_velocity = 0.0;
    emulator.status.jog_rate = 0.0;
    emulator.status.feed_rate = 0.0;
    emulator.status.motion_mode = None;
    emulator.dwell_remaining = 0.0;

    if emulator.status.interpreter_state == InterpreterState::Reading {
        emulator.status.interpreter_state = InterpreterState::Paused;
    }

    emulator.status.message = Some("Motion stopped".into());
    Ok(Value::Null)
}

/// Start (or, with `direction == 0`, decelerate out of) a continuous jog on
/// one axis at the requested feed rate, clamped to the axis maximum.
pub(super) fn jog_continuous(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    emulator.ensure_motion_allowed()?;
    let axis: Axis = serde_json::from_value(args.get("axis").cloned().unwrap_or(Value::Null))
        .map_err(|error| api_error("bad_request", error.to_string()))?;
    let direction = args.get("direction").and_then(Value::as_i64).unwrap_or(0) as i8;
    let feed = args.get("feed_rate").and_then(Value::as_f64).unwrap_or(0.0);

    if direction == 0 {
        begin_jog_deceleration(emulator);
    } else if ![-1, 1].contains(&direction) || !feed.is_finite() || feed <= 0.0 {
        return Err(api_error("bad_jog", "invalid continuous jog parameters"));
    } else {
        let feed = feed.min(emulator.config.axis_limits(axis).2);
        emulator.jog_stop = None;
        emulator.continuous_jog_xy = None;
        emulator.continuous_jog = Some((axis, direction, feed));
        emulator.status.feed_rate = 0.0;
        emulator.status.motion_mode = Some("Jog".into());
    }
    Ok(Value::Null)
}

/// Start (or stop, when both directions are zero) a continuous XY jog. The
/// effective feed is reduced so neither axis exceeds its own maximum when
/// the diagonal motion is decomposed into components.
pub(super) fn jog_continuous_xy(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    emulator.ensure_motion_allowed()?;
    let x_direction = args.get("x_direction").and_then(Value::as_i64).unwrap_or(0) as i8;
    let y_direction = args.get("y_direction").and_then(Value::as_i64).unwrap_or(0) as i8;
    let feed = args.get("feed_rate").and_then(Value::as_f64).unwrap_or(0.0);

    if ![-1, 0, 1].contains(&x_direction)
        || ![-1, 0, 1].contains(&y_direction)
        || !feed.is_finite()
        || feed <= 0.0
    {
        return Err(api_error("bad_jog", "invalid continuous XY jog parameters"));
    }

    if x_direction == 0 && y_direction == 0 {
        begin_jog_deceleration(emulator);
    } else {
        if emulator.active_move.is_some() || emulator.status.homing {
            return Err(api_error(
                "motion_busy",
                "another movement is already active",
            ));
        }

        let norm = f64::from(x_direction * x_direction + y_direction * y_direction).sqrt();
        let mut max_feed = feed;
        if x_direction != 0 {
            max_feed = max_feed
                .min(emulator.config.axis_limits(Axis::X).2 * norm / f64::from(x_direction.abs()));
        }
        if y_direction != 0 {
            max_feed = max_feed
                .min(emulator.config.axis_limits(Axis::Y).2 * norm / f64::from(y_direction.abs()));
        }

        emulator.jog_stop = None;
        emulator.continuous_jog = None;
        emulator.continuous_jog_xy = Some((x_direction, y_direction, max_feed));
        emulator.status.feed_rate = 0.0;
        emulator.status.motion_mode = Some("Jog".into());
    }
    Ok(Value::Null)
}
