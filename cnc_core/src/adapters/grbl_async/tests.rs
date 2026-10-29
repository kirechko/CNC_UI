//! Unit tests for the command/transport layer of the GRBL adapter:
//! spindle normalization and limits, jog command formatting, the guard
//! helpers, program-line validation and the RX line extractor.
//!
//! This file *is* the `grbl_async::tests` module (declared in `mod.rs`), so
//! `use super::*;` pulls in the private types, constants and `pub(super)`
//! helpers of the adapter module tree. Status-report parsing tests live in
//! the sibling `status_tests` module.

use super::*;
use crate::{ControllerError, SpindleDirection};

/// Shared fixture: a clockwise 1200 RPM spindle setpoint used by the
/// status-parsing tests.
fn spindle_command() -> SpindleCommand {
    SpindleCommand {
        direction: SpindleDirection::Clockwise,
        rpm: 1200.0,
    }
}

#[test]
fn detects_coordinate_system_changes_in_gcode_without_comment_matches() {
    assert_eq!(
        GrblAdapter::coordinate_system_select("G55 G0 X1"),
        Some(CoordinateSystem::G55)
    );
    assert_eq!(
        GrblAdapter::coordinate_system_select("G54 G1X1 G59.3"),
        Some(CoordinateSystem::G59_3)
    );
    assert_eq!(
        GrblAdapter::coordinate_system_select("G1 X1 (G55) ; G56"),
        None
    );
    assert_eq!(GrblAdapter::coordinate_system_select("G54.1 P1"), None);
}

#[test]
fn stopped_or_zero_speed_spindle_commands_are_normalized() {
    let stopped = GrblAdapter::normalize_spindle_command(SpindleCommand {
        direction: SpindleDirection::Clockwise,
        rpm: 0.0,
    });
    assert_eq!(stopped.direction, SpindleDirection::Stopped);
    assert_eq!(stopped.rpm, 0.0);

    let zero_direction = GrblAdapter::normalize_spindle_command(SpindleCommand {
        direction: SpindleDirection::Stopped,
        rpm: 1200.0,
    });
    assert_eq!(zero_direction.direction, SpindleDirection::Stopped);
    assert_eq!(zero_direction.rpm, 0.0);

    let running = spindle_command();
    assert_eq!(GrblAdapter::normalize_spindle_command(running), running);
}

#[test]
fn spindle_commands_respect_reported_controller_limits() {
    let command = SpindleCommand {
        direction: SpindleDirection::Clockwise,
        rpm: 1200.0,
    };

    assert!(GrblAdapter::validate_spindle_command(command, Some(500.0), Some(24000.0)).is_ok());
    assert_eq!(
        GrblAdapter::validate_spindle_command(command, Some(1500.0), Some(24000.0))
            .unwrap_err()
            .code,
        "spindle_limit"
    );
    assert_eq!(
        GrblAdapter::validate_spindle_command(command, Some(500.0), Some(1000.0))
            .unwrap_err()
            .code,
        "spindle_limit"
    );

    let stopped = SpindleCommand {
        direction: SpindleDirection::Stopped,
        rpm: 0.0,
    };
    assert!(GrblAdapter::validate_spindle_command(stopped, Some(500.0), Some(24000.0)).is_ok());
}

#[test]
fn axis_jog_command_preserves_metric_units_and_numeric_precision() {
    let command = GrblAdapter::format_axis_jog_command(Axis::X, 0.00001, 0.25).unwrap();

    assert_eq!(command, "$J=G21 G91 X0.00001 F0.25");
}

#[test]
fn xy_jog_command_explicitly_uses_metric_units() {
    let command = GrblAdapter::format_xy_jog_command(1, -1, 1200.0).unwrap();

    assert_eq!(command, "$J=G21 G91 X7.0711 Y-7.0711 F1200");
    assert!(GrblAdapter::format_xy_jog_command(0, 0, 1200.0).is_err());
}

#[test]
fn axis_jog_command_rejects_zero_or_non_finite_values() {
    assert!(GrblAdapter::format_axis_jog_command(Axis::X, 0.0, 100.0).is_err());
    assert!(GrblAdapter::format_axis_jog_command(Axis::X, f64::NAN, 100.0).is_err());
    assert!(GrblAdapter::format_axis_jog_command(Axis::X, 1.0, 0.0).is_err());
    assert!(GrblAdapter::format_axis_jog_command(Axis::X, 1.0, f64::INFINITY).is_err());
}

#[test]
fn rejects_non_ascii_and_control_characters_in_commands() {
    for command in ["G1 X1\tY2", "G1 X1\0", "G1 X1 °"] {
        let error = GrblAdapter::validate_command(command).unwrap_err();
        assert_eq!(error.code, "bad_command", "command: {command:?}");
    }
}

#[test]
fn prevents_manual_commands_during_program_streaming() {
    assert!(GrblAdapter::ensure_manual_command_allowed(false).is_ok());

    let error = GrblAdapter::ensure_manual_command_allowed(true).unwrap_err();
    assert_eq!(error.code, "motion_busy");
}

#[test]
fn idle_detection_requires_an_exact_valid_state_name() {
    assert!(GrblAdapter::is_idle_status("<Idle|MPos:0,0,0>"));
    assert!(GrblAdapter::is_idle_status("<idle|MPos:0,0,0>"));
    assert!(!GrblAdapter::is_idle_status("<Idleish|MPos:0,0,0>"));
    assert!(!GrblAdapter::is_idle_status("<Run|MPos:0,0,0>"));
    assert!(!GrblAdapter::is_idle_status("Idle|MPos:0,0,0"));
}

#[test]
fn program_streaming_continues_only_in_idle_run_or_check_states() {
    // `Check` must keep streaming so the standard `$C` dry-run workflow
    // (verify G-code without motion) can push the whole program through.
    for state in ["Idle", "Run", "Check", "idle", "run", "check"] {
        assert!(
            GrblAdapter::should_stream_program(&format!("<{state}|MPos:0,0,0>")),
            "expected streaming in state {state}"
        );
    }

    for state in ["Hold:0", "Door:1", "Jog", "Alarm:1", "Sleep"] {
        assert!(
            !GrblAdapter::should_stream_program(&format!("<{state}|MPos:0,0,0>")),
            "expected streaming to pause in state {state}"
        );
    }
}

#[test]
fn only_starts_program_when_grbl_reports_idle() {
    assert!(GrblAdapter::ensure_controller_idle("<Idle|MPos:0,0,0>").is_ok());

    for state in ["Run", "Jog", "Hold:0", "Door:1", "Alarm:1"] {
        let response = format!("<{state}|MPos:0,0,0>");
        let error = GrblAdapter::ensure_controller_idle(&response).unwrap_err();
        assert_eq!(error.code, "motion_busy", "state: {state}");
    }

    assert_eq!(
        GrblAdapter::ensure_controller_idle("<Unknown|MPos:0,0,0>")
            .unwrap_err()
            .code,
        "protocol"
    );
}

#[test]
fn prevents_conflicting_commands_during_continuous_jog() {
    assert!(GrblAdapter::ensure_jog_idle(false).is_ok());

    let error = GrblAdapter::ensure_jog_idle(true).unwrap_err();
    assert_eq!(error.code, "motion_busy");
}

#[test]
fn prevents_program_load_or_start_during_program_or_jog_motion() {
    assert!(GrblAdapter::ensure_program_idle(false, false).is_ok());

    for (program_streaming, jog_active) in [(true, false), (false, true), (true, true)] {
        let error = GrblAdapter::ensure_program_idle(program_streaming, jog_active).unwrap_err();
        assert_eq!(error.code, "motion_busy");
    }
}

#[test]
fn rejects_programs_over_the_size_limit() {
    let content = " ".repeat(MAX_PROGRAM_BYTES + 1);
    let error = GrblAdapter::parse_program_lines(&content).unwrap_err();

    assert_eq!(error.code, "bad_program");
    assert!(error.message.contains("size limit"));
}

#[test]
fn unsupported_coolant_mcodes_are_skipped_during_streaming() {
    let unsupported = ControllerError::new("gcode_error", "GRBL rejected command (20): M7");
    // Bare coolant words (with or without zero padding) are optional firmware
    // features on stock GRBL builds, so their rejection must not abort a cut.
    for command in ["M7", "M8", "M9", "m7", "M08", " M9 "] {
        assert!(
            GrblAdapter::is_tolerated_accessory_rejection(command, &unsupported),
            "`{command}` should be tolerated"
        );
    }
    // Spindle and motion words are never tolerated, and only error 20 qualifies.
    assert!(!GrblAdapter::is_tolerated_accessory_rejection(
        "M3",
        &unsupported
    ));
    assert!(!GrblAdapter::is_tolerated_accessory_rejection(
        "M70",
        &unsupported
    ));
    let alarm = ControllerError::new("alarm", "ALARM:1");
    assert!(!GrblAdapter::is_tolerated_accessory_rejection("M7", &alarm));
}

#[test]
fn program_lines_drop_semicolon_comments_before_validation() {
    // `;` comments (even with non-ASCII text such as Cyrillic) are stripped
    // host-side: GRBL ignores them anyway, and the strict ASCII validator must
    // never see them, otherwise real-world programs fail to load.
    let lines =
        GrblAdapter::parse_program_lines("; Настройки\nG21 ; Миллиметры\nM7  ; охлаждение\nG0 X1")
            .unwrap();
    assert_eq!(lines, ["G21", "M7", "G0 X1"]);

    // A `;` inside a parenthesis comment is not a line comment and stays, so
    // the whole line still fails the ASCII check exactly like before.
    let error = GrblAdapter::parse_program_lines("G0 X1 (a ; b ü)").unwrap_err();
    assert_eq!(error.code, "bad_program");
}

#[test]
fn validates_every_program_line_before_execution() {
    let lines = GrblAdapter::parse_program_lines("\nG0 X1\n\nG1 X2\n").unwrap();
    assert_eq!(lines, ["G0 X1", "G1 X2"]);

    let error = GrblAdapter::parse_program_lines(&format!(
        "G0 X1\n{}\nG1 X2",
        "G".repeat(MAX_COMMAND_LENGTH + 1)
    ))
    .unwrap_err();

    assert_eq!(error.code, "bad_program");
    assert!(error.message.contains("line 2"));
}

#[test]
fn validates_grbl_command_line_length_and_single_line_format() {
    assert!(GrblAdapter::validate_command("G1 X10 Y20").is_ok());
    assert!(GrblAdapter::validate_command(&"G".repeat(MAX_COMMAND_LENGTH)).is_ok());

    let error = GrblAdapter::validate_command(&"G".repeat(MAX_COMMAND_LENGTH + 1)).unwrap_err();
    assert_eq!(error.code, "bad_command");

    let error = GrblAdapter::validate_command("G1 X1\nM3 S1000").unwrap_err();
    assert_eq!(error.code, "bad_command");
}

#[test]
fn rejects_operations_after_protocol_timeout() {
    let error = GrblAdapter::ensure_protocol_usable(true).unwrap_err();
    assert_eq!(error.code, "protocol_desynchronized");
    assert!(GrblAdapter::ensure_protocol_usable(false).is_ok());
}

#[test]
fn extracts_complete_line_and_preserves_following_buffered_lines() {
    let mut buffer = b"ok\n<Idle|MPos:1,2,3|FS:0,0>\n".to_vec();

    assert_eq!(
        GrblAdapter::extract_line(&mut buffer).unwrap().as_deref(),
        Some("ok")
    );
    assert_eq!(
        GrblAdapter::extract_line(&mut buffer).unwrap().as_deref(),
        Some("<Idle|MPos:1,2,3|FS:0,0>")
    );
    assert!(buffer.is_empty());
}

#[test]
fn rejects_non_utf8_response_lines() {
    let mut buffer = vec![0xff, b'\n'];

    let error = GrblAdapter::extract_line(&mut buffer).unwrap_err();
    assert_eq!(error.code, "protocol");
}

#[test]
fn rejects_oversized_line_even_when_terminator_is_already_buffered() {
    let mut buffer = vec![b'x'; MAX_RESPONSE_LINE + 1];
    buffer.push(b'\n');

    let error = GrblAdapter::extract_line(&mut buffer).unwrap_err();
    assert_eq!(error.code, "protocol");
}
