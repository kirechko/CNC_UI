//! Unit tests for Klipper/Moonraker status mapping, RPC URL handling, and capabilities.

use super::{
    capabilities::detect_laser_pin,
    status::{map_status, subscription_objects},
    Session,
};
use serde_json::{json, Map, Value};

/// Builds a disconnected protocol session with deterministic printer-object state.
fn session_with(objects: &[(&str, Value)]) -> Session {
    Session {
        socket: None,
        next_id: 1,
        objects: objects
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect::<Map<_, _>>(),
        object_names: Vec::new(),
        machine_name: None,
        config_settings: Value::Null,
        laser_pin: None,
        spindle_command: crate::SpindleCommand {
            direction: crate::SpindleDirection::Stopped,
            rpm: 0.0,
        },
        program_name: None,
        toolpath: crate::Toolpath::default(),
        total_program_lines: 0,
        script_line: 0,
        continuous_jog: None,
        subscribed: false,
        estopped: false,
    }
}

#[test]
fn status_maps_feed_units_homing_and_program_progress() {
    let mut session = session_with(&[
        ("webhooks", json!({"state":"ready"})),
        (
            "print_stats",
            json!({"state":"printing", "filename":"part.gcode"}),
        ),
        ("motion_report", json!({"live_position":[10.0, 20.0, 3.0]})),
        (
            "gcode_move",
            json!({"gcode_position":[9.0, 18.0, 2.0], "homing_origin":[1.0,2.0,1.0], "speed":25.0, "speed_factor":1.2}),
        ),
        ("toolhead", json!({"homed_axes":"xyz"})),
        (
            "virtual_sdcard",
            json!({"progress":0.5,"file_position":200}),
        ),
    ]);
    session.total_program_lines = 100;
    let status = map_status(&session, true).expect("status mapping");
    assert!(status.connected);
    assert_eq!(status.feed_rate, 1500.0);
    assert_eq!(status.position.x, 10.0);
    assert_eq!(status.work_offset.y, 2.0);
    assert_eq!(status.homed, [true, true, true, false, false, false]);
    assert_eq!(status.loaded_program.as_deref(), Some("part.gcode"));
    assert_eq!(status.current_line, 50);
    assert_eq!(status.progress.program_line, Some(50));
    assert_eq!(status.progress.tool_position.unwrap().x, 9.0);
}

#[test]
fn gcode_error_capture_clears_cached_laser_command() {
    let mut session = session_with(&[("webhooks", json!({"state":"ready"}))]);
    session.laser_pin = Some("laser".into());
    session.spindle_command = crate::SpindleCommand {
        direction: crate::SpindleDirection::Clockwise,
        rpm: 0.6,
    };
    assert!(super::rpc::capture_gcode_error(
        &mut session,
        "!! Move out of range"
    ));
    assert_eq!(
        session.objects.get("__error").and_then(Value::as_str),
        Some("!! Move out of range")
    );
    assert_eq!(
        session.spindle_command.direction,
        crate::SpindleDirection::Stopped
    );
    assert!(!super::rpc::capture_gcode_error(&mut session, "ok"));
}

#[test]
fn subscription_includes_discovered_macros_and_output_objects() {
    let names = vec![
        "gcode_macro M3".into(),
        "output_pin laser".into(),
        "pwm_tool spindle".into(),
        "fan_generic fan".into(),
    ];
    let subscribed = subscription_objects(&names);
    assert!(subscribed.contains(&"gcode_macro M3".to_owned()));
    assert!(subscribed.contains(&"output_pin laser".to_owned()));
    assert!(subscribed.contains(&"pwm_tool spindle".to_owned()));
    assert!(!subscribed.contains(&"fan_generic fan".to_owned()));
}

#[test]
fn capabilities_detect_single_pin_and_extract_limits() {
    let settings = json!({
        "stepper_x":{"position_min":"-10", "position_max":"200"},
        "printer":{"max_velocity":"300", "max_accel":2500},
        "pwm_tool":{"laser":{"cycle_time":0.0001}}
    });
    assert_eq!(detect_laser_pin(&settings).as_deref(), Some("laser"));
    let configuration = super::KlipperAdapter::configuration_from_settings(&settings);
    assert_eq!(configuration.axes[0].max_travel, Some(210.0));
    assert_eq!(configuration.axes[0].max_velocity, Some(18000.0));
    assert!(configuration.spindle.laser.unwrap().dynamic_power_supported);
}

#[test]
fn gcode_macro_objects_are_included_in_status_subscriptions() {
    let names = vec!["gcode_macro M3".into(), "gcode_macro CUSTOM_PROBE".into()];
    let subscribed = subscription_objects(&names);
    assert!(subscribed.contains(&"gcode_macro M3".to_owned()));
    assert!(subscribed.contains(&"gcode_macro CUSTOM_PROBE".to_owned()));
}

#[test]
fn delta_stepper_carriage_limits_are_not_mapped_as_xyz_travel() {
    let settings = json!({"printer":{"kinematics":"delta","max_velocity":200},"stepper_x":{"position_min":-120,"position_max":120}});
    let configuration = super::KlipperAdapter::configuration_from_settings(&settings);
    assert_eq!(configuration.axes[0].max_travel, None);
}

#[test]
fn rpc_url_normalizes_http_schemes_and_paths() {
    assert_eq!(
        super::rpc::test_websocket_url("http://printer.local:7125"),
        "ws://printer.local:7125/websocket"
    );
    assert_eq!(
        super::rpc::test_websocket_url("https://printer.local/moonraker"),
        "wss://printer.local/moonraker/websocket"
    );
}

#[test]
fn disconnected_snapshot_is_not_ready() {
    let snapshot = super::disconnected_status();
    assert!(!snapshot.connected);
    assert_eq!(snapshot.task_state, crate::TaskState::Off);
}
