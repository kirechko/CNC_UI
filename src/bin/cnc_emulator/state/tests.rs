//! Tests emulator soft limits and program-loading behavior.
use super::super::gcode::test_emulator;
use super::super::protocol::Request;

#[test]
fn target_inside_limits_is_accepted() {
    assert!(test_emulator()
        .check_target([150.0, 200.0, 50.0, 0.0, 0.0])
        .is_ok());
}

#[test]
fn exact_axis_boundaries_are_accepted() {
    let emulator = test_emulator();
    assert!(emulator.check_target([0.0, 0.0, 0.0, 0.0, 0.0]).is_ok());
    assert!(emulator
        .check_target([300.0, 400.0, 100.0, 0.0, 0.0])
        .is_ok());
}

#[test]
fn targets_below_axis_minima_are_rejected() {
    let emulator = test_emulator();
    assert!(emulator
        .check_target([-0.001, 100.0, 50.0, 0.0, 0.0])
        .is_err());
    assert!(emulator
        .check_target([100.0, -0.001, 50.0, 0.0, 0.0])
        .is_err());
    assert!(emulator
        .check_target([100.0, 100.0, -0.001, 0.0, 0.0])
        .is_err());
}

#[test]
fn targets_above_axis_maxima_are_rejected() {
    let emulator = test_emulator();
    assert!(emulator
        .check_target([300.001, 100.0, 50.0, 0.0, 0.0])
        .is_err());
    assert!(emulator
        .check_target([100.0, 400.001, 50.0, 0.0, 0.0])
        .is_err());
    assert!(emulator
        .check_target([100.0, 100.0, 100.001, 0.0, 0.0])
        .is_err());
}

#[test]
fn small_floating_point_tolerance_is_allowed() {
    let emulator = test_emulator();
    assert!(emulator
        .check_target([-5e-9, 100.0, 50.0, 0.0, 0.0])
        .is_ok());
    assert!(emulator
        .check_target([300.0 + 5e-9, 100.0, 50.0, 0.0, 0.0])
        .is_ok());
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

/// Laser-profile fixture used to validate immediate S-value telemetry and safety limits.
fn laser_emulator() -> super::super::Emulator {
    let mut emulator = test_emulator();
    emulator.config.spindle.mode = crate::cnc_api::SpindleMode::LaserConstant;
    emulator.config.spindle.laser = Some(crate::cnc_api::LaserConfiguration {
        min_power: 0.0,
        max_power: 255.0,
        pwm_frequency_hz: Some(5000.0),
        dynamic_power_supported: true,
        auto_shutoff_on_pause: true,
    });
    emulator.status.spindle.mode = crate::cnc_api::SpindleMode::LaserConstant;
    emulator.status.spindle.laser_power = Some(0.0);
    emulator
}

/// Ensures laser commands bypass spindle acceleration and report native S power immediately.
#[test]
fn laser_output_uses_immediate_native_power_and_m5_is_fail_closed() {
    let mut emulator = laser_emulator();
    // Emulator API requests use immediate adapter commands, whereas G-code S alone
    // is modal and intentionally does not enable output. Exercise the M3 state path directly.
    emulator.set_spindle(crate::cnc_api::SpindleDirection::Clockwise, Some(40.0));
    assert!(emulator.status.spindle.enabled);
    assert_eq!(emulator.status.spindle.actual_rpm, 40.0);
    assert_eq!(emulator.status.spindle.laser_power, Some(40.0));
    emulator.set_spindle(crate::cnc_api::SpindleDirection::Stopped, Some(0.0));
    assert!(!emulator.status.spindle.enabled);
    assert_eq!(emulator.status.spindle.laser_power, Some(0.0));
}

/// Rejects an S value above the explicit laser PWM range before changing output state.
#[test]
fn laser_output_rejects_out_of_range_power_without_emitting() {
    let mut emulator = laser_emulator();
    assert!(emulator.execute_gcode("S256").is_err());
    assert!(!emulator.status.spindle.enabled);
    assert_eq!(emulator.status.spindle.laser_power, Some(0.0));
}

/// Verifies pause, program abort, and E-stop force laser output telemetry off.
#[test]
fn laser_output_shuts_off_on_pause_abort_and_estop() {
    let mut emulator = laser_emulator();
    emulator.set_spindle(crate::cnc_api::SpindleDirection::Clockwise, Some(30.0));
    emulator.status.interpreter_state = crate::cnc_api::InterpreterState::Reading;
    emulator
        .handle(Request {
            method: "program_pause".into(),
            args: serde_json::json!({}),
        })
        .unwrap();
    assert!(!emulator.status.spindle.enabled);
    assert_eq!(emulator.status.spindle.laser_power, Some(0.0));
    emulator.set_spindle(crate::cnc_api::SpindleDirection::Clockwise, Some(20.0));
    emulator
        .handle(Request {
            method: "program_abort".into(),
            args: serde_json::json!({}),
        })
        .unwrap();
    assert!(!emulator.status.spindle.enabled);
    emulator.set_spindle(crate::cnc_api::SpindleDirection::Clockwise, Some(10.0));
    emulator
        .handle(Request {
            method: "estop".into(),
            args: serde_json::json!({}),
        })
        .unwrap();
    assert!(!emulator.status.spindle.enabled);
    assert_eq!(emulator.status.spindle.laser_power, Some(0.0));
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

/// Exercises drilling moves, tool changes, and an operator pause in one emulator program.
#[test]
fn substituted_drilling_program_runs_with_simulated_tool_changes() {
    let mut emulator = test_emulator();
    emulator.status.position.z = 1.0;
    // Plain G0/G1 blocks model the emitted replacement for a canned drill cycle.
    emulator.program = vec![
        "G21 G90".into(),
        "T2 M6".into(),
        "G0 X5 Y5 Z5".into(),
        "G1 Z10 F120".into(),
        "G0 Z5".into(),
        "M0".into(),
        "M30".into(),
    ];
    emulator.status.total_lines = emulator.program.len() as u32;
    emulator.status.interpreter_state = crate::cnc_api::InterpreterState::Reading;

    for _ in 0..10_000 {
        emulator.tick(0.01);
        if emulator.status.interpreter_state == crate::cnc_api::InterpreterState::Paused {
            break;
        }
    }

    assert_eq!(
        emulator.status.interpreter_state,
        crate::cnc_api::InterpreterState::Paused
    );
    assert_eq!(emulator.status.current_line, 6);
    assert_eq!(
        emulator.status.message.as_deref(),
        Some("Program paused for operator (M0)")
    );
    assert_eq!(emulator.status.total_lines, 7);
    assert_eq!(emulator.selected_tool, Some(2));
    assert_eq!(emulator.current_tool, Some(2));
    assert_eq!(emulator.status.selected_tool, Some(2));
    assert_eq!(emulator.status.current_tool, Some(2));
    assert_eq!(emulator.status.position.z, 5.0);
}

/// Confirms the probe reaches the emulated stock plane and reports its contact point.
#[test]
fn g38_probe_macro_measures_the_virtual_stock_surface() {
    let mut emulator = test_emulator();
    emulator.absolute_mode = false;
    emulator.status.position.z = 60.0;
    emulator.execute_gcode("G38.2 Z-20 F100").unwrap();

    assert_eq!(emulator.active_move.as_ref().map(|_| true), Some(true));
    assert!(emulator.status.probe_triggered);
    let mut movement = emulator.active_move.take().unwrap();
    for _ in 0..1000 {
        let (position, complete) = movement.tick(0.01);
        if complete {
            assert!((position[2] - 50.0).abs() < 1e-6);
            return;
        }
    }
    panic!("probe movement did not reach the contact plane");
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
