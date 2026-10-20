//! Implements the emulator's API commands and request dispatch.
use super::super::dynamics::{ActiveMove, Motion};
use super::super::protocol::{api_error, Request, WireError};
use super::Emulator;
use crate::cnc_api::*;
use serde_json::Value;

pub(super) fn handle(emulator: &mut Emulator, request: Request) -> Result<Value, WireError> {
    if request.method != "status" {
        let mut logged_args = request.args.clone();

        // Не засоряем терминал содержимым загружаемой программы.
        if request.method == "load_program" {
            if let Some(object) = logged_args.as_object_mut() {
                if let Some(content) = object.remove("content") {
                    let byte_count = content.as_str().map(str::len).unwrap_or(0);
                    object.insert("content_bytes".into(), Value::from(byte_count as u64));
                }
            }
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let log_source = "эмулятор/API";
        let log_message = format!("{} {}", request.method, logged_args);
        crate::message_log::record("информация", log_source, &log_message);
        eprintln!("[API {timestamp}] {} {}", request.method, logged_args);
    }

    let args = request.args;
    match request.method.as_str() {
        "status" => Ok(serde_json::to_value(&emulator.status)
            .map_err(|error| api_error("serialization", error.to_string()))?),
        "machine_configuration" => Ok(serde_json::to_value(
            emulator.config.machine_configuration(),
        )
        .map_err(|error| api_error("serialization", error.to_string()))?),
        "power_on" => power_on(emulator),
        "power_off" => power_off(emulator, false),
        "estop" => power_off(emulator, true),
        "select_coordinate_system" => select_coordinate_system(emulator, args),
        "set_work_zero" => set_work_zero(emulator, args),
        "set_axis_work_zero" => set_axis_work_zero(emulator, args),
        "home" => home(emulator, args),
        "jog_increment" => jog_increment(emulator, args),
        "jog_continuous" => jog_continuous(emulator, args),
        "jog_continuous_xy" => jog_continuous_xy(emulator, args),
        "jog_cancel" => jog_cancel(emulator),
        "stop_motion" => stop_motion(emulator),
        "mdi" => mdi(emulator, args),
        "load_program" => load_program(emulator, args),
        "program_run" => program_run(emulator, args),
        "program_pause" => program_pause(emulator),
        "program_resume" => program_resume(emulator),
        "program_abort" => program_abort(emulator),
        "set_spindle" => set_spindle(emulator, args),
        "clear_error" => clear_error(emulator),
        _ => Err(api_error(
            "unknown_method",
            format!("unsupported method: {}", request.method),
        )),
    }
}

pub(super) fn mdi(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    emulator.ensure_motion_allowed()?;
    emulator.status.task_mode = TaskMode::Mdi;
    emulator.execute_gcode(args.get("command").and_then(Value::as_str).unwrap_or(""))?;
    Ok(Value::Null)
}

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

pub(super) fn program_pause(emulator: &mut Emulator) -> Result<Value, WireError> {
    if emulator.status.interpreter_state == InterpreterState::Reading
        && !emulator.stop_retract_active
    {
        emulator.status.interpreter_state = InterpreterState::Paused;
        emulator.status.message = Some("Program paused".into());
    }
    Ok(Value::Null)
}

pub(super) fn program_resume(emulator: &mut Emulator) -> Result<Value, WireError> {
    if emulator.status.interpreter_state == InterpreterState::Paused {
        emulator.status.interpreter_state = InterpreterState::Reading;
    }
    Ok(Value::Null)
}

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

    // STOP принудительно гасит шпиндель, не выполняя ramp-down.
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

pub(super) fn clear_error(emulator: &mut Emulator) -> Result<Value, WireError> {
    if emulator.status.task_state == TaskState::Estop {
        emulator.status.task_state = TaskState::EstopReset;
        emulator.status.message = Some("Alarm reset".into());
    } else {
        emulator.status.message = None;
    }
    Ok(Value::Null)
}

pub(super) fn power_on(emulator: &mut Emulator) -> Result<Value, WireError> {
    emulator.status.task_state = TaskState::On;
    emulator.status.message = Some("Machine powered on".into());
    Ok(Value::Null)
}

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

pub(super) fn set_work_zero(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    let system: crate::cnc_api::CoordinateSystem =
        serde_json::from_value(args.get("system").cloned().unwrap_or(Value::Null))
            .map_err(|error| api_error("bad_request", error.to_string()))?;

    emulator.set_selected_work_zero(system)?;
    Ok(Value::Null)
}

pub(super) fn set_axis_work_zero(emulator: &mut Emulator, args: Value) -> Result<Value, WireError> {
    let axis: Axis = serde_json::from_value(args.get("axis").cloned().unwrap_or(Value::Null))
        .map_err(|error| api_error("bad_request", error.to_string()))?;

    emulator.set_axis_work_zero(axis)?;
    Ok(Value::Null)
}

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
        return Err(api_error("unsupported_axis", "the emulator does not configure a C axis"));
    }
    if ![-1, 1].contains(&request.direction)
        || !request.distance.is_finite()
        || request.distance < 0.0
    {
        return Err(api_error("bad_jog", "invalid jog direction or distance"));
    }

    let index = super::axis_index(request.axis);
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

fn begin_jog_deceleration(emulator: &mut Emulator) {
    let motion = if let Some((axis, direction, _)) = emulator.continuous_jog.take() {
        let mut vector = [0.0; 5];
        vector[super::axis_index(axis)] = f64::from(direction);
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
        return Err(api_error("unsupported_axis", "the emulator does not configure a C axis"));
    }

    emulator.homing_queue = match requested {
        Some(axis) => vec![axis],
        None => emulator.config.configured_axes(),
    };
    for axis in &emulator.homing_queue {
        emulator.status.homed[super::axis_index(*axis)] = false;
    }

    emulator.status.homing = true;
    emulator.status.motion_mode = Some("Homing".into());
    emulator.status.task_mode = crate::cnc_api::TaskMode::Manual;
    emulator.status.message = Some("Homing started".into());
    Ok(Value::Null)
}

impl Emulator {
    pub(super) fn ensure_motion_allowed(&self) -> Result<(), WireError> {
        if self.status.task_state != TaskState::On {
            return Err(api_error(
                "machine_off",
                "virtual machine is not powered on",
            ));
        }
        Ok(())
    }
}
