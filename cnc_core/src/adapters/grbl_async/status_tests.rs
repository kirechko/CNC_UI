//! Unit tests for the realtime status-report parser (`status.rs`) and the
//! axis/coordinate-system guards that the status pipeline depends on.
//!
//! These tests exercise `GrblAdapter::parse_status` through its full public
//! signature (cached work offset, active coordinate system, spindle setpoint,
//! program cursor), so they cover offset derivation, telemetry validation and
//! GRBL state subcodes. Command-layer tests live in the sibling `tests`
//! module; both share `use super::*;` to reach the private adapter items.

use super::*;
use crate::{InterpreterState, SpindleDirection};

/// Shared fixture: a clockwise 1200 RPM spindle setpoint, matching the
/// original single test module's helper.
fn spindle_command() -> SpindleCommand {
    SpindleCommand {
        direction: SpindleDirection::Clockwise,
        rpm: 1200.0,
    }
}

#[test]
fn parses_machine_position_and_derives_work_offset() {
    let status = GrblAdapter::parse_status(
        "<Idle|MPos:10.000,20.000,30.000|WPos:1.000,2.000,3.000|FS:100.0,0.0>",
        None,
        CoordinateSystem::G55,
        spindle_command(),
        Some("part.ngc"),
        2,
        5,
    )
    .unwrap();

    assert_eq!(status.position.x, 10.0);
    assert_eq!(status.position.y, 20.0);
    assert_eq!(status.position.z, 30.0);
    assert_eq!(status.work_offset.x, 9.0);
    assert_eq!(status.work_offset.y, 18.0);
    assert_eq!(status.work_offset.z, 27.0);
    assert_eq!(status.active_coordinate_system, CoordinateSystem::G55);
    assert_eq!(status.loaded_program.as_deref(), Some("part.ngc"));
    assert_eq!((status.current_line, status.total_lines), (2, 5));
}

#[test]
fn reuses_cached_work_offset_when_grbl_omits_wco() {
    let status = GrblAdapter::parse_status(
        "<Idle|WPos:1.000,2.000,3.000|FS:0,0>",
        Some([9.0, 18.0, 27.0]),
        CoordinateSystem::G54,
        spindle_command(),
        None,
        0,
        0,
    )
    .unwrap();

    assert_eq!(status.position.x, 10.0);
    assert_eq!(status.position.y, 20.0);
    assert_eq!(status.position.z, 30.0);
    assert_eq!(status.work_offset.x, 9.0);
    assert_eq!(status.work_offset.y, 18.0);
    assert_eq!(status.work_offset.z, 27.0);
}

#[test]
fn reported_work_coordinate_offset_takes_precedence() {
    let status = GrblAdapter::parse_status(
        "<Idle|MPos:10,20,30|WPos:1,2,3|WCO:8,17,26|FS:0,0>",
        None,
        CoordinateSystem::G54,
        spindle_command(),
        None,
        0,
        0,
    )
    .unwrap();

    assert_eq!(status.work_offset.x, 8.0);
    assert_eq!(status.work_offset.y, 17.0);
    assert_eq!(status.work_offset.z, 26.0);
}

#[test]
fn continuous_xy_stop_does_not_require_a_positive_feed_rate() {
    assert!(GrblAdapter::validate_continuous_xy_jog(0, 0, 0.0).is_ok());
    assert!(GrblAdapter::validate_continuous_xy_jog(0, 0, f64::NAN).is_ok());
    assert!(GrblAdapter::validate_continuous_xy_jog(1, 0, 0.0).is_err());
    assert!(GrblAdapter::validate_continuous_xy_jog(2, 0, 100.0).is_err());
}

#[test]
fn rejects_coordinate_systems_not_supported_by_grbl() {
    for system in [
        CoordinateSystem::G59_1,
        CoordinateSystem::G59_2,
        CoordinateSystem::G59_3,
    ] {
        let error = GrblAdapter::ensure_supported_coordinate_system(system).unwrap_err();
        assert_eq!(error.code, "unsupported_coordinate_system");
    }

    for system in [
        CoordinateSystem::G54,
        CoordinateSystem::G55,
        CoordinateSystem::G56,
        CoordinateSystem::G57,
        CoordinateSystem::G58,
        CoordinateSystem::G59,
    ] {
        assert!(GrblAdapter::ensure_supported_coordinate_system(system).is_ok());
    }
}

#[test]
fn rejects_rotary_axes_for_grbl_jog() {
    for axis in [Axis::A, Axis::B] {
        let error = GrblAdapter::ensure_supported_axis(axis).unwrap_err();
        assert_eq!(error.code, "unsupported_axis");
    }

    for axis in [Axis::X, Axis::Y, Axis::Z] {
        assert!(GrblAdapter::ensure_supported_axis(axis).is_ok());
    }
}

#[test]
fn parses_feed_only_status_reports() {
    let status = GrblAdapter::parse_status(
        "<Idle|MPos:1,2,3|F:120.5>",
        None,
        CoordinateSystem::G54,
        spindle_command(),
        None,
        0,
        0,
    )
    .unwrap();

    assert_eq!(status.feed_rate, 120.5);
    assert_eq!(status.spindle.actual_rpm, 0.0);
}

#[test]
fn rejects_duplicate_feed_telemetry_fields() {
    for response in [
        "<Idle|MPos:0,0,0|F:100|F:200>",
        "<Idle|MPos:0,0,0|F:100|FS:200,300>",
        "<Idle|MPos:0,0,0|FS:100,300|F:200>",
    ] {
        let error = GrblAdapter::parse_status(
            response,
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap_err();

        assert_eq!(error.code, "protocol", "response: {response}");
    }
}

#[test]
fn rejects_malformed_feed_and_spindle_telemetry() {
    for response in [
        "<Idle|MPos:0,0,0|FS:bad,0>",
        "<Idle|MPos:0,0,0|FS:100,NaN>",
        "<Idle|MPos:0,0,0|FS:-1,0>",
        "<Idle|MPos:0,0,0|FS:100,0,extra>",
    ] {
        let error = GrblAdapter::parse_status(
            response,
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap_err();

        assert_eq!(error.code, "protocol", "response: {response}");
    }
}

#[test]
fn rejects_duplicate_position_and_telemetry_fields() {
    for response in [
        "<Idle|MPos:1,2,3|MPos:4,5,6|WCO:0,0,0|FS:0,0>",
        "<Idle|WPos:1,2,3|WPos:4,5,6|WCO:0,0,0|FS:0,0>",
        "<Idle|MPos:1,2,3|WCO:0,0,0|WCO:1,1,1|FS:0,0>",
        "<Idle|MPos:1,2,3|WCO:0,0,0|FS:0,0|FS:1,1>",
    ] {
        let error = GrblAdapter::parse_status(
            response,
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap_err();

        assert_eq!(error.code, "protocol", "response: {response}");
    }
}

#[test]
fn rejects_malformed_reported_coordinate_fields() {
    for response in [
        "<Idle|MPos:1,NaN,3|FS:0,0>",
        "<Idle|WPos:1,2|FS:0,0>",
        "<Idle|WCO:1,2,Infinity|FS:0,0>",
    ] {
        assert!(
            GrblAdapter::parse_status(
                response,
                None,
                CoordinateSystem::G54,
                spindle_command(),
                None,
                0,
                0,
            )
            .is_err(),
            "expected malformed status field to be rejected: {response}"
        );
    }
}

#[test]
fn rejects_overflow_when_deriving_offsets_or_machine_positions() {
    for (response, cached_offset) in [
        ("<Idle|MPos:1e308,0,0|WPos:-1e308,0,0|FS:0,0>", None),
        ("<Idle|WPos:1e308,0,0|FS:0,0>", Some([1e308, 0.0, 0.0])),
    ] {
        let error = GrblAdapter::parse_status(
            response,
            cached_offset,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap_err();

        assert_eq!(error.code, "protocol");
    }
}

#[test]
fn validates_grbl_state_subcodes() {
    for state in [
        "Idle", "Run", "Jog", "Check", "Home", "Sleep", "Hold:0", "Hold:1", "Door:0", "Door:3",
        "Alarm", "Alarm:1",
    ] {
        assert!(GrblAdapter::is_known_machine_state(state), "state: {state}");
    }

    for state in [
        "Idle:1",
        "Run:0",
        "Hold:2",
        "Door:4",
        "Door:invalid",
        "Alarm:0",
        "Alarm:invalid",
    ] {
        assert!(
            !GrblAdapter::is_known_machine_state(state),
            "state: {state}"
        );
    }
}

#[test]
fn rejects_unknown_or_empty_grbl_machine_states() {
    for response in [
        "<|MPos:0,0,0|FS:0,0>",
        "<Unknown|MPos:0,0,0|FS:0,0>",
        "<Idleish|MPos:0,0,0|FS:0,0>",
        "<Runaway|MPos:0,0,0|FS:0,0>",
        "<Hold:invalid|MPos:0,0,0|FS:0,0>",
    ] {
        let error = GrblAdapter::parse_status(
            response,
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap_err();

        assert_eq!(error.code, "protocol", "response: {response}");
    }
}

#[test]
fn accepts_machine_position_when_grbl_omits_work_offset() {
    let status = GrblAdapter::parse_status(
        "<Idle|MPos:1,2,3|FS:0,0>",
        None,
        CoordinateSystem::G54,
        spindle_command(),
        None,
        0,
        0,
    )
    .unwrap();

    assert_eq!(status.position.x, 1.0);
    assert_eq!(status.position.y, 2.0);
    assert_eq!(status.position.z, 3.0);
    assert_eq!(status.work_offset.x, 0.0);
    assert_eq!(status.work_offset.y, 0.0);
    assert_eq!(status.work_offset.z, 0.0);
}

#[test]
fn rejects_work_position_when_no_offset_is_available() {
    let error = GrblAdapter::parse_status(
        "<Idle|WPos:1,2,3|FS:0,0>",
        None,
        CoordinateSystem::G54,
        spindle_command(),
        None,
        0,
        0,
    )
    .unwrap_err();

    assert_eq!(error.code, "protocol");
    assert!(error.message.contains("work coordinate offset"));
}

#[test]
fn maps_grbl_door_interlock_to_paused_state() {
    let status = GrblAdapter::parse_status(
        "<Door:1|MPos:1,2,3|WCO:0,0,0|FS:0,0>",
        None,
        CoordinateSystem::G54,
        spindle_command(),
        None,
        0,
        0,
    )
    .unwrap();

    assert_eq!(status.interpreter_state, InterpreterState::Paused);
    assert_eq!(status.message.as_deref(), Some("GRBL Door:1"));
}

#[test]
fn rejects_status_without_position_fields() {
    let error = GrblAdapter::parse_status(
        "<Idle|FS:0,0>",
        None,
        CoordinateSystem::G54,
        spindle_command(),
        None,
        0,
        0,
    )
    .unwrap_err();

    assert_eq!(error.code, "protocol");
}

#[test]
fn rejects_malformed_status_and_non_finite_coordinates() {
    assert!(GrblAdapter::parse_status(
        "Idle|MPos:1,2,3",
        None,
        CoordinateSystem::G54,
        spindle_command(),
        None,
        0,
        0,
    )
    .is_err());

    assert!(GrblAdapter::parse_triplet("1,NaN,3").is_none());
    assert!(GrblAdapter::parse_triplet("1,2").is_none());
}
