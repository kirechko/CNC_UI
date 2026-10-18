 //! Tests emulator soft limits and program-loading behavior.
use super::super::gcode::test_emulator;
use super::super::protocol::Request;

#[test]
fn target_inside_limits_is_accepted() {
    assert!(test_emulator().check_target([150.0, 200.0, 50.0, 0.0, 0.0]).is_ok());
}

#[test]
fn exact_axis_boundaries_are_accepted() {
    let emulator = test_emulator();
    assert!(emulator.check_target([0.0, 0.0, 0.0, 0.0, 0.0]).is_ok());
    assert!(emulator.check_target([300.0, 400.0, 100.0, 0.0, 0.0]).is_ok());
}

#[test]
fn targets_below_axis_minima_are_rejected() {
    let emulator = test_emulator();
    assert!(emulator.check_target([-0.001, 100.0, 50.0, 0.0, 0.0]).is_err());
    assert!(emulator.check_target([100.0, -0.001, 50.0, 0.0, 0.0]).is_err());
    assert!(emulator.check_target([100.0, 100.0, -0.001, 0.0, 0.0]).is_err());
}

#[test]
fn targets_above_axis_maxima_are_rejected() {
    let emulator = test_emulator();
    assert!(emulator.check_target([300.001, 100.0, 50.0, 0.0, 0.0]).is_err());
    assert!(emulator.check_target([100.0, 400.001, 50.0, 0.0, 0.0]).is_err());
    assert!(emulator.check_target([100.0, 100.0, 100.001, 0.0, 0.0]).is_err());
}

#[test]
fn small_floating_point_tolerance_is_allowed() {
    let emulator = test_emulator();
    assert!(emulator.check_target([-5e-9, 100.0, 50.0, 0.0, 0.0]).is_ok());
    assert!(emulator.check_target([300.0 + 5e-9, 100.0, 50.0, 0.0, 0.0]).is_ok());
}

#[test]
fn incremental_jog_motion_ramps_up_under_configured_acceleration() {
    let mut emulator = test_emulator();
    let request = Request {
        method: "jog_increment".into(),
        args: serde_json::json!({
            "request": {
                "axis": "X",
                "direction": 1,
                "distance": 10.0,
                "feed_rate": 1200.0
            }
        }),
    };

    emulator.handle(request).unwrap();
    let mut movement = emulator.active_move.take().unwrap();

    assert_eq!(movement.velocity_mm_min(), 0.0);
    movement.tick(0.01);

    assert!(movement.velocity_mm_min() > 0.0);
    assert!(movement.velocity_mm_min() < 1200.0);
    assert!((movement.velocity_mm_min() - 60.0).abs() < 1e-6);
}

#[test]
fn spindle_ramps_to_commanded_speed_and_reports_telemetry() {
    let mut emulator = test_emulator();
    let request = Request {
        method: "set_spindle".into(),
        args: serde_json::json!({
            "command": {
                "direction": "Clockwise",
                "rpm": 1200.0
            }
        }),
    };

    emulator.handle(request).unwrap();
    assert_eq!(emulator.status.spindle.commanded_rpm, 1200.0);

    emulator.tick(0.1);
    assert_eq!(emulator.status.spindle.actual_rpm, 100.0);
    assert!(emulator.status.spindle.enabled);

    for _ in 0..11 {
        emulator.tick(0.1);
    }

    assert_eq!(emulator.status.spindle.actual_rpm, 1200.0);
    let telemetry = serde_json::to_value(&emulator.status).unwrap();
    assert_eq!(telemetry["spindle"]["commanded_rpm"], 1200.0);
    assert_eq!(telemetry["spindle"]["actual_rpm"], 1200.0);
    assert_eq!(telemetry["spindle"]["enabled"], true);
}

#[test]
fn load_program_strips_semicolon_comments_but_keeps_parenthesis_comments() {
    let mut emulator = test_emulator();

    let request = Request {
        method: "load_program".into(),
        args: serde_json::json!({
            "name": "comments.ngc",
            "content": "G1 X10 ; M8 X20\nG1 X30 (G99 Y42)\n"
        }),
    };

    emulator.handle(request).unwrap();

    assert_eq!(
        emulator.program,
        vec!["G1 X10".to_string(), "G1 X30 (G99 Y42)".to_string(),]
    );
}
