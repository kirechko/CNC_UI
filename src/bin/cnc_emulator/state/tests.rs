use super::super::gcode::test_emulator;
use super::super::protocol::Request;

#[test]
fn target_inside_limits_is_accepted() {
    assert!(test_emulator().check_target([150.0, 200.0, 50.0]).is_ok());
}

#[test]
fn exact_axis_boundaries_are_accepted() {
    let emulator = test_emulator();
    assert!(emulator.check_target([0.0, 0.0, 0.0]).is_ok());
    assert!(emulator.check_target([300.0, 400.0, 100.0]).is_ok());
}

#[test]
fn targets_below_axis_minima_are_rejected() {
    let emulator = test_emulator();
    assert!(emulator.check_target([-0.001, 100.0, 50.0]).is_err());
    assert!(emulator.check_target([100.0, -0.001, 50.0]).is_err());
    assert!(emulator.check_target([100.0, 100.0, -0.001]).is_err());
}

#[test]
fn targets_above_axis_maxima_are_rejected() {
    let emulator = test_emulator();
    assert!(emulator.check_target([300.001, 100.0, 50.0]).is_err());
    assert!(emulator.check_target([100.0, 400.001, 50.0]).is_err());
    assert!(emulator.check_target([100.0, 100.0, 100.001]).is_err());
}

#[test]
fn small_floating_point_tolerance_is_allowed() {
    let emulator = test_emulator();
    assert!(emulator.check_target([-5e-9, 100.0, 50.0]).is_ok());
    assert!(emulator.check_target([300.0 + 5e-9, 100.0, 50.0]).is_ok());
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
        vec![
            "G1 X10".to_string(),
            "G1 X30 (G99 Y42)".to_string(),
        ]
    );
}
