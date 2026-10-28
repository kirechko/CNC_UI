//! Unit tests for the toolpath/progress contract: nearest-segment locating,
//! line-preferring and forward-only searches, the `/api/toolpath` JSON shape
//! and round-trip serialization. Bodies are verbatim from the original inline
//! `mod tests` of the pre-split `toolpath.rs`.

use super::*;

#[test]
fn unavailable_progress_serializes_as_nulls() {
    let value = serde_json::to_value(ToolpathProgress::unavailable()).unwrap();

    assert_eq!(
        value,
        serde_json::json!({
            "program_line": null,
            "path_distance_mm": null,
            "tool_position": null
        })
    );
}

#[test]
fn progress_deserializes_from_older_payloads_without_fields() {
    let progress: ToolpathProgress = serde_json::from_value(serde_json::json!({})).unwrap();

    assert!(!progress.is_available());
}

#[test]
fn default_toolpath_is_empty() {
    let toolpath = Toolpath::default();

    assert!(toolpath.segments.is_empty());
    assert_eq!(toolpath.total_length_mm, 0.0);
}

/// Verifies that `locate_on_line` prefers the executing line when a shared
/// vertex belongs to two segments, while plain `locate` takes the first one.
#[test]
fn locate_on_line_prefers_executing_line_at_shared_vertex() {
    let toolpath = l_shaped_toolpath();
    let vertex = pos(10.0, 0.0, 0.0);

    let unrestricted = toolpath.locate(&vertex).unwrap();
    assert_eq!(unrestricted.program_line, Some(1));

    let on_second_line = toolpath.locate_on_line(&vertex, 2).unwrap();
    assert_eq!(on_second_line.program_line, Some(2));
    assert_close(on_second_line.path_distance_mm.unwrap(), 10.0);
}

/// Verifies that `locate_on_line` falls back to the whole path when the
/// executing line has no segment near the tool.
#[test]
fn locate_on_line_falls_back_to_whole_path_when_line_is_far() {
    let toolpath = l_shaped_toolpath();
    let on_first = toolpath.locate_on_line(&pos(5.0, 0.0, 0.0), 2).unwrap();
    assert_eq!(on_first.program_line, Some(1));
    assert_close(on_first.path_distance_mm.unwrap(), 5.0);
}

/// Reproduces the lift after a circular pass: the tool rises straight up from
/// the arc end at (8.4125, 0), which shares XY with earlier passes. The
/// reported line must be the lift line (`G0 Z10`), not an earlier arc line.
#[test]
fn lift_after_arc_pass_reports_lift_line_not_earlier_pass() {
    let source = "G0 Z10\n G0 X8.4125 Y0\nG1 Z-2\nG2 X8.4125 Y0 I-8.4125 J0\nG1 Z-4\nG2 X8.4125 Y0 I-8.4125 J0\nG0 Z10\n";
    let toolpath = crate::toolpath_parser::parse_toolpath(source);
    let lift_top = pos(8.4125, 0.0, 10.0);

    let progress = toolpath.locate_on_line(&lift_top, 7).unwrap();
    assert_eq!(progress.program_line, Some(7));
}

/// Builds a position with zeroed rotary axes for compact test fixtures.
#[test]
fn forward_search_ignores_earlier_pass_under_same_xy_column() {
    // The lift (line 7) shares its XY column with the earlier passes.
    // Forward search from the lift start must report the lift, not a pass.
    let source = "G0 Z10\n G0 X8.4125 Y0\nG1 Z-2\nG2 X8.4125 Y0 I-8.4125 J0\nG1 Z-4\nG2 X8.4125 Y0 I-8.4125 J0\nG0 Z10\n";
    let toolpath = crate::toolpath_parser::parse_toolpath(source);
    let lift_start = toolpath.segments.last().unwrap().start_distance_mm;
    let on_earlier_pass = pos(8.4125, 0.0, -2.0);
    let progress = toolpath
        .locate_forward(&on_earlier_pass, lift_start, 7)
        .unwrap();
    assert_eq!(progress.program_line, Some(7));
}

#[test]
fn forward_search_respects_max_program_line() {
    // Line 1 (`G0 Z10` from the origin) is too far from the lift top, and
    // the lift top is only reachable on lines 2 and 7 of this source.
    let source = "G0 Z10\n G0 X8.4125 Y0\nG1 Z-2\nG2 X8.4125 Y0 I-8.4125 J0\nG1 Z-4\nG2 X8.4125 Y0 I-8.4125 J0\nG0 Z10\n";
    let toolpath = crate::toolpath_parser::parse_toolpath(source);
    let lift_top = pos(8.4125, 0.0, 10.0);
    assert!(toolpath.locate_forward(&lift_top, 0.0, 1).is_none());
}

fn pos(x: f64, y: f64, z: f64) -> crate::Position {
    crate::Position {
        x,
        y,
        z,
        a: 0.0,
        b: 0.0,
        c: 0.0,
    }
}

/// Asserts that two floating-point values match within a small tolerance.
fn assert_close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

/// Builds a two-segment L-shaped toolpath: 10 mm along X, then 10 mm along Y.
fn l_shaped_toolpath() -> Toolpath {
    Toolpath {
        segments: vec![
            ToolpathSegment {
                from: pos(0.0, 0.0, 0.0),
                to: pos(10.0, 0.0, 0.0),
                kind: MotionKind::Linear,
                program_line: 1,
                feed_rate: 1000.0,
                start_distance_mm: 0.0,
                length_mm: 10.0,
            },
            ToolpathSegment {
                from: pos(10.0, 0.0, 0.0),
                to: pos(10.0, 10.0, 0.0),
                kind: MotionKind::Linear,
                program_line: 2,
                feed_rate: 1000.0,
                start_distance_mm: 10.0,
                length_mm: 10.0,
            },
        ],
        total_length_mm: 20.0,
    }
}

#[test]
fn locate_on_first_segment_interior_returns_projected_distance() {
    let toolpath = l_shaped_toolpath();
    let progress = toolpath.locate(&pos(5.0, 0.0, 0.0)).unwrap();

    assert_eq!(progress.program_line, Some(1));
    assert_close(progress.path_distance_mm.unwrap(), 5.0);
    assert_eq!(progress.tool_position, Some(pos(5.0, 0.0, 0.0)));
}

#[test]
fn locate_on_second_segment_adds_cumulative_start_distance() {
    let toolpath = l_shaped_toolpath();
    let progress = toolpath.locate(&pos(10.0, 5.0, 0.0)).unwrap();

    assert_eq!(progress.program_line, Some(2));
    assert_close(progress.path_distance_mm.unwrap(), 15.0);
}

#[test]
fn locate_at_path_end_returns_total_length() {
    let toolpath = l_shaped_toolpath();
    let progress = toolpath.locate(&pos(10.0, 10.0, 0.0)).unwrap();

    assert_eq!(progress.program_line, Some(2));
    assert_close(progress.path_distance_mm.unwrap(), 20.0);
}

#[test]
fn locate_off_path_reports_no_distance_beyond_tolerance() {
    let toolpath = l_shaped_toolpath();
    // 3 mm away from the first segment: outside the tolerance, so no traversed distance.
    let progress = toolpath.locate(&pos(5.0, 3.0, 0.0)).unwrap();

    assert_eq!(progress.program_line, None);
    assert_eq!(progress.path_distance_mm, None);
    assert_eq!(progress.tool_position, Some(pos(5.0, 3.0, 0.0)));
}

#[test]
fn locate_far_from_path_reports_no_traversed_distance() {
    let toolpath = l_shaped_toolpath();
    // 5 mm away from the path (JOG): no traversed distance must be reported.
    let progress = toolpath.locate(&pos(5.0, 5.0, 0.0)).unwrap();

    assert_eq!(progress.program_line, None);
    assert_eq!(progress.path_distance_mm, None);
    assert_eq!(progress.tool_position, Some(pos(5.0, 5.0, 0.0)));
}

#[test]
fn locate_within_tolerance_still_reports_distance() {
    let toolpath = l_shaped_toolpath();
    let progress = toolpath.locate(&pos(5.0, PATH_TOLERANCE_MM * 0.5, 0.0)).unwrap();

    assert_eq!(progress.program_line, Some(1));
    assert_close(progress.path_distance_mm.unwrap(), 5.0);
}

#[test]
fn locate_beyond_segment_end_clamps_to_segment_boundary() {
    let toolpath = l_shaped_toolpath();
    // Just past the end of the first segment along X (within tolerance),
    // the nearest point is the segment end, so the distance is clamped to 10 mm.
    let progress = toolpath.locate(&pos(10.05, 0.0, 0.0)).unwrap();

    assert_eq!(progress.program_line, Some(1));
    assert_close(progress.path_distance_mm.unwrap(), 10.0);
}

#[test]
fn locate_on_empty_toolpath_returns_none() {
    assert!(Toolpath::default().locate(&pos(1.0, 2.0, 3.0)).is_none());
}

/// Documents the known limitation: on a self-crossing path, a position at the
/// crossing point resolves to the first matching segment, not the later pass.
#[test]
fn locate_on_self_crossing_path_returns_first_matching_segment() {
    // Path: X 0->10 (line 1), then Y 0->10 (line 2), crossing at (5, 5)? Use
    // a crossing at (5,0): second pass runs along X back through (5,0).
    let toolpath = Toolpath {
        segments: vec![
            ToolpathSegment {
                from: pos(0.0, 0.0, 0.0),
                to: pos(10.0, 0.0, 0.0),
                kind: MotionKind::Linear,
                program_line: 1,
                feed_rate: 1000.0,
                start_distance_mm: 0.0,
                length_mm: 10.0,
            },
            ToolpathSegment {
                from: pos(10.0, 0.0, 0.0),
                to: pos(5.0, 0.0, 0.0),
                kind: MotionKind::Linear,
                program_line: 2,
                feed_rate: 1000.0,
                start_distance_mm: 10.0,
                length_mm: 5.0,
            },
        ],
        total_length_mm: 15.0,
    };
    // (5, 0, 0) lies on both passes; the first segment wins on ties.
    let progress = toolpath.locate(&pos(5.0, 0.0, 0.0)).unwrap();

    assert_eq!(progress.program_line, Some(1));
    assert_close(progress.path_distance_mm.unwrap(), 5.0);
}

/// Verifies the `/api/toolpath` JSON shape that the web UI relies on:
/// snake_case motion kinds, endpoint objects and distance field names.
#[test]
fn toolpath_serializes_with_frontend_field_names() {
    let value = serde_json::to_value(l_shaped_toolpath()).unwrap();
    let first = &value["segments"][0];

    assert_eq!(first["kind"], "linear");
    assert_eq!(first["program_line"], 1);
    assert_eq!(first["feed_rate"], 1000.0);
    assert_eq!(first["start_distance_mm"], 0.0);
    assert_eq!(first["length_mm"], 10.0);
    assert_eq!(first["from"]["x"], 0.0);
    assert_eq!(first["to"]["x"], 10.0);
    assert_eq!(value["total_length_mm"], 20.0);
}

/// Verifies that a toolpath JSON payload round-trips without loss.
#[test]
fn toolpath_round_trips_through_json() {
    let original = l_shaped_toolpath();
    let json = serde_json::to_string(&original).unwrap();
    let restored: Toolpath = serde_json::from_str(&json).unwrap();

    assert_eq!(restored, original);
}
