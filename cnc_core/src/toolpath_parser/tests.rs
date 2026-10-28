//! Unit tests for the G-code toolpath parser, ported verbatim from the original
//! inline `mod tests` of the pre-split `toolpath_parser.rs`.

use super::lex::to_position;
use super::*;
use crate::toolpath::ToolpathSegment;
use crate::MotionKind;

/// Asserts two floats are equal within a small tolerance.
fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-6,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn empty_source_produces_empty_toolpath() {
    let toolpath = parse_toolpath("");

    assert!(toolpath.segments.is_empty());
    assert_eq!(toolpath.total_length_mm, 0.0);
}

#[test]
fn comments_and_blank_lines_are_ignored_and_not_counted() {
    let toolpath = parse_toolpath("(setup)\n\n; only comment\nG1 X10 ; move\n  \nG1 Y5\n");

    assert_eq!(toolpath.segments.len(), 2);
    // Program lines count only executable lines: first move is 1, second is 2.
    assert_eq!(toolpath.segments[0].program_line, 1);
    assert_eq!(toolpath.segments[1].program_line, 2);
}

#[test]
fn crlf_line_endings_are_handled() {
    let toolpath = parse_toolpath("G1 X10\r\nG1 X20\r\n");

    assert_eq!(toolpath.segments.len(), 2);
    assert_close(toolpath.total_length_mm, 20.0);
}

#[test]
fn rapid_and_linear_motion_kinds_are_tracked() {
    let toolpath = parse_toolpath("G0 X10\nG1 X20 F600\n");

    assert_eq!(toolpath.segments[0].kind, MotionKind::Rapid);
    assert_eq!(toolpath.segments[1].kind, MotionKind::Linear);
    assert_close(toolpath.segments[1].feed_rate, 600.0);
}

#[test]
fn modal_motion_carries_over_to_lines_without_g_word() {
    let toolpath = parse_toolpath("G1 X10 F300\nX20\n");

    assert_eq!(toolpath.segments.len(), 2);
    assert_eq!(toolpath.segments[1].kind, MotionKind::Linear);
}

#[test]
fn relative_mode_adds_to_current_position() {
    let toolpath = parse_toolpath("G91\nG1 X5\nG1 X5\n");

    assert_close(toolpath.segments[1].to.x, 10.0);
    assert_close(toolpath.total_length_mm, 10.0);
}

#[test]
fn inch_units_scale_coordinates_and_feed() {
    let toolpath = parse_toolpath("G20\nG1 X1 F10\n");

    assert_close(toolpath.segments[0].to.x, 25.4);
    assert_close(toolpath.segments[0].feed_rate, 254.0);
    assert_close(toolpath.total_length_mm, 25.4);
}

#[test]
fn cumulative_distance_is_consistent_across_segments() {
    let toolpath = parse_toolpath("G1 X3 Y4\nG1 X3 Y8\n");

    assert_close(toolpath.segments[0].length_mm, 5.0);
    assert_close(toolpath.segments[1].start_distance_mm, 5.0);
    assert_close(toolpath.total_length_mm, 9.0);
}

#[test]
fn arc_is_split_into_chords_with_clockwise_kind() {
    // Quarter circle, radius 10, CW from (10,0) to (0,-10) around origin.
    let toolpath = parse_toolpath("G1 X10 Y0\nG2 X0 Y-10 I-10 J0\n");

    let arc: Vec<&ToolpathSegment> = toolpath
        .segments
        .iter()
        .filter(|s| s.kind == MotionKind::ArcClockwise)
        .collect();
    assert!(arc.len() >= MIN_ARC_CHORDS);
    let last = arc.last().unwrap();
    assert_close(last.to.x, 0.0);
    assert_close(last.to.y, -10.0);
}

#[test]
fn full_circle_when_end_equals_start() {
    let toolpath = parse_toolpath("G1 X8 Y0\nG2 X8 Y0 I-8 J0\n");

    let arc_length: f64 = toolpath
        .segments
        .iter()
        .filter(|s| s.kind == MotionKind::ArcClockwise)
        .map(|s| s.length_mm)
        .sum();
    // Full circle of radius 8 has circumference 2*pi*8.
    assert!((arc_length - std::f64::consts::TAU * 8.0).abs() < 0.5);
}

#[test]
fn arc_without_center_words_is_not_an_arc() {
    let toolpath = parse_toolpath("G1 X0 Y0\nG2 X10 Y0\n");

    assert!(toolpath
        .segments
        .iter()
        .all(|s| s.kind != MotionKind::ArcClockwise));
}

#[test]
fn exponent_numbers_are_parsed() {
    let toolpath = parse_toolpath("G1 X1.5e1\n");

    assert_close(toolpath.segments[0].to.x, 15.0);
}

/// Regression: adapters call `locate` on the parsed toolpath. On a parsed arc,
/// the end position must report the full cumulative length, and a point in the
/// middle of the arc must report a distance between the arc start and end.
#[test]
fn locate_on_parsed_arc_reports_arc_distance() {
    // G1 X10 Y0 (10 mm along X), then CW quarter arc radius 10 to (0,-10).
    let toolpath = parse_toolpath("G1 X10 Y0\nG2 X0 Y-10 I-10 J0\n");
    let quarter_arc = std::f64::consts::FRAC_PI_2 * 10.0;

    // Arc end: whole path is traversed, executed on program line 2.
    let end = toolpath.locate(&to_position([0.0, -10.0, 0.0])).unwrap();
    assert_eq!(end.program_line, Some(2));
    assert!(
        (end.path_distance_mm.unwrap() - toolpath.total_length_mm).abs() < 1e-6,
        "end distance should equal total length"
    );
    assert!((toolpath.total_length_mm - (10.0 + quarter_arc)).abs() < 0.5);

    // Arc midpoint at 45 degrees: 10 mm of line plus half of the quarter arc.
    let mid_angle = std::f64::consts::FRAC_PI_4;
    let mid = toolpath.locate(&to_position([10.0 * mid_angle.cos(), -10.0 * mid_angle.sin(), 0.0])).unwrap();
    assert_eq!(mid.program_line, Some(2));
    let expected_mid = 10.0 + quarter_arc / 2.0;
    assert!(
        (mid.path_distance_mm.unwrap() - expected_mid).abs() < 0.5,
        "mid-arc distance {:?} should be near {expected_mid}",
        mid.path_distance_mm
    );
}
