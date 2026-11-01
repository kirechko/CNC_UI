//! Moonraker object subscription and translation into the shared machine status.

use crate::MachineConfiguration;
use crate::{
    CoordinateSystem, CoordinateSystemOffset, InterpreterState, MachineStatus, Position,
    SpindleDirection, SpindleStatus, TaskMode, TaskState, ToolpathProgress,
};
use serde_json::{Map, Value};

/// Builds the subscribed object list, including discovered macro/pin objects.
pub(super) fn subscription_objects(names: &[String]) -> Vec<String> {
    let mut objects = [
        "webhooks",
        "print_stats",
        "gcode_move",
        "toolhead",
        "motion_report",
        "idle_timeout",
        "configfile",
        "virtual_sdcard",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    objects.extend(
        names
            .iter()
            .filter(|name| {
                name.starts_with("gcode_macro ")
                    || name.starts_with("output_pin ")
                    || name.starts_with("pwm_tool ")
            })
            .cloned(),
    );
    objects.sort();
    objects.dedup();
    objects
}

/// Maps Moonraker printer objects into a complete UI status snapshot.
pub(super) fn map_status(
    session: &super::Session,
    connected: bool,
) -> crate::CncResult<MachineStatus> {
    let object = |name: &str| session.objects.get(name).and_then(Value::as_object);
    let number = |object: Option<&Map<String, Value>>, key: &str| {
        object
            .and_then(|o| o.get(key))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    let position = |values: Option<&Vec<Value>>| {
        let values = values.cloned().unwrap_or_default();
        Position {
            x: values.first().and_then(Value::as_f64).unwrap_or(0.0),
            y: values.get(1).and_then(Value::as_f64).unwrap_or(0.0),
            z: values.get(2).and_then(Value::as_f64).unwrap_or(0.0),
            a: values.get(3).and_then(Value::as_f64).unwrap_or(0.0),
            b: values.get(4).and_then(Value::as_f64).unwrap_or(0.0),
            c: values.get(5).and_then(Value::as_f64).unwrap_or(0.0),
        }
    };
    let motion = object("motion_report");
    let move_obj = object("gcode_move");
    let machine = position(
        motion
            .and_then(|o| o.get("live_position"))
            .and_then(Value::as_array),
    );
    let work = position(
        move_obj
            .and_then(|o| o.get("gcode_position"))
            .and_then(Value::as_array),
    );
    let origin = position(
        move_obj
            .and_then(|o| o.get("homing_origin"))
            .and_then(Value::as_array),
    );
    let webhooks_state = object("webhooks")
        .and_then(|o| o.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("disconnected");
    let print_state = object("print_stats")
        .and_then(|o| o.get("state"))
        .and_then(Value::as_str)
        .unwrap_or("standby");
    let controller_ready = connected && webhooks_state.eq_ignore_ascii_case("ready");
    let running = print_state.eq_ignore_ascii_case("printing");
    let paused = print_state.eq_ignore_ascii_case("paused");
    let homed_text = object("toolhead")
        .and_then(|o| o.get("homed_axes"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let homed = ["x", "y", "z", "a", "b", "c"].map(|axis| {
        homed_text
            .chars()
            .any(|item| item.eq_ignore_ascii_case(&axis.chars().next().unwrap_or_default()))
    });
    let velocity = number(move_obj, "speed");
    let feed_factor = number(move_obj, "speed_factor");
    let filename = object("print_stats")
        .and_then(|o| o.get("filename"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let file_progress = object("virtual_sdcard")
        .and_then(|o| o.get("progress"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let estimated_line = (session.total_program_lines > 0)
        .then_some((file_progress * f64::from(session.total_program_lines)).round() as u32);
    let error_message = session
        .objects
        .get("__error")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let path_progress = if session.toolpath.segments.is_empty() {
        ToolpathProgress {
            tool_position: Some(work),
            ..ToolpathProgress::default()
        }
    } else {
        session
            .toolpath
            .locate(&work)
            .unwrap_or_else(|| ToolpathProgress {
                program_line: None,
                path_distance_mm: None,
                tool_position: Some(work),
            })
    };
    Ok(MachineStatus {
        connected: controller_ready,
        task_mode: if running || paused {
            TaskMode::Auto
        } else {
            TaskMode::Manual
        },
        task_state: if webhooks_state.eq_ignore_ascii_case("shutdown")
            || webhooks_state.eq_ignore_ascii_case("error")
        {
            TaskState::Estop
        } else if controller_ready {
            TaskState::On
        } else {
            TaskState::Off
        },
        interpreter_state: if running {
            InterpreterState::Reading
        } else if paused {
            InterpreterState::Paused
        } else {
            InterpreterState::Idle
        },
        position: machine,
        work_offset: origin,
        active_coordinate_system: CoordinateSystem::G54,
        coordinate_systems: vec![CoordinateSystemOffset {
            system: CoordinateSystem::G54,
            offset: origin,
        }],
        feed_rate: velocity * 60.0,
        spindle: spindle_status(session),
        mist_coolant_on: false,
        flood_coolant_on: false,
        jog_rate: if running { 0.0 } else { velocity * 60.0 },
        motion_mode: if running { Some("G-code".into()) } else { None },
        max_spindle_rpm: 0.0,
        probe_triggered: false,
        probe_position: None,
        selected_tool: None,
        current_tool: None,
        loaded_program: filename.or_else(|| session.program_name.clone()),
        current_line: estimated_line.unwrap_or(0),
        total_lines: session.total_program_lines,
        message: error_message.or_else(|| {
            Some(format!(
                "Klipper {webhooks_state}; print {print_state}; feed override {:.0}%",
                feed_factor * 100.0
            ))
        }),
        homed,
        homing: false,
        progress: if running || paused {
            ToolpathProgress {
                program_line: path_progress.program_line.or(estimated_line),
                path_distance_mm: path_progress.path_distance_mm,
                tool_position: Some(work),
            }
        } else {
            ToolpathProgress::default()
        },
    })
}

/// Builds the configured axes and spindle capabilities from the subscribed Moonraker configfile object.
pub(super) fn configuration_snapshot(session: &super::Session) -> MachineConfiguration {
    super::KlipperAdapter::configuration_from_settings(&session.config_settings)
}

/// Reads the active PWM/output pin's reported value when Moonraker supplies it.
fn spindle_status(session: &super::Session) -> SpindleStatus {
    let pin_value = session
        .laser_pin
        .as_ref()
        .and_then(|pin| {
            session
                .objects
                .get(&format!("pwm_tool {pin}"))
                .or_else(|| session.objects.get(&format!("output_pin {pin}")))
        })
        .and_then(|value| value.get("value"))
        .and_then(Value::as_f64);
    let enabled = pin_value.unwrap_or(session.spindle_command.rpm) > 0.0;
    SpindleStatus {
        commanded_rpm: session.spindle_command.rpm,
        actual_rpm: pin_value.unwrap_or(0.0),
        direction: if enabled {
            SpindleDirection::Clockwise
        } else {
            SpindleDirection::Stopped
        },
        enabled,
        mode: if session.laser_pin.is_some() {
            crate::SpindleMode::LaserDynamic
        } else {
            crate::SpindleMode::Standard
        },
        laser_power: session.laser_pin.as_ref().and(pin_value),
    }
}
