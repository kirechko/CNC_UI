//! Behavioral regression tests for affine mapping and toolpath rewriting.

use super::*;
use crate::error::RosettaError;
use crate::io::gcode::GcodeIo;
use crate::io::ToolpathIo;
use crate::ir::toolpath::{Point2, Point3, SegmentKind, Toolpath, Units};
use crate::transform::model::{MirrorAxes, Region, Transform, TransformKind};
use crate::util::almost_equal;

/// 90° CCW rotation around the origin.
fn rotate90() -> AffineTransform {
    AffineTransform::new(TransformKind::Rotate {
        angle_deg: 90.0,
        center: Point2::new(0.0, 0.0),
    })
}

#[test]
fn rotate_90_golden() {
    let map = AffineMap::from_kind(&TransformKind::Rotate {
        angle_deg: 90.0,
        center: Point2::new(0.0, 0.0),
    })
    .unwrap();
    let p = map.map_xy(Point2::new(10.0, 0.0));
    assert!(
        almost_equal(p.x, 0.0, 1e-9) && almost_equal(p.y, 10.0, 1e-9),
        "{p:?}"
    );
    // Rotation around a nonzero center: (11,1) about (1,1) → (1, 11).
    let map = AffineMap::from_kind(&TransformKind::Rotate {
        angle_deg: 90.0,
        center: Point2::new(1.0, 1.0),
    })
    .unwrap();
    let p = map.map_xy(Point2::new(11.0, 1.0));
    assert!(
        almost_equal(p.x, 1.0, 1e-9) && almost_equal(p.y, 11.0, 1e-9),
        "{p:?}"
    );
}

#[test]
fn rotate_toolpath_segments_and_reports_lines() {
    let mut tp = GcodeIo::new().parse("G0 X10 Y0\nG1 X10 Y5 F100\n").unwrap();
    let report = rotate90().apply(&mut tp, &Region::Whole).unwrap();
    assert_eq!(report.affected_segments, 2);
    assert_eq!(report.affected_lines, vec![1, 2]);
    assert!(almost_equal(tp.segments[0].to.x, 0.0, 1e-9));
    assert!(almost_equal(tp.segments[0].to.y, 10.0, 1e-9));
    assert!(almost_equal(tp.segments[1].to.x, -5.0, 1e-9));
    assert!(almost_equal(tp.segments[1].to.y, 10.0, 1e-9));
}

#[test]
fn mirror_flips_arc_direction_xy_mirror_keeps_it() {
    let mut tp = GcodeIo::new().parse("G2 X10 Y0 I5 J0\n").unwrap();
    let mirror_x = AffineTransform::new(TransformKind::Mirror {
        axes: MirrorAxes::X,
    });
    let report = mirror_x.apply(&mut tp, &Region::Whole).unwrap();
    assert_eq!(tp.segments[0].kind, SegmentKind::ArcCcw);
    assert!(report.notes.iter().any(|n| n.contains("flipped")));

    let mut tp2 = GcodeIo::new().parse("G2 X10 Y0 I5 J0\n").unwrap();
    let mirror_xy = AffineTransform::new(TransformKind::Mirror {
        axes: MirrorAxes::Xy,
    });
    mirror_xy.apply(&mut tp2, &Region::Whole).unwrap();
    // 180° rotation: a mirrored mirror keeps the original direction.
    assert_eq!(tp2.segments[0].kind, SegmentKind::ArcCw);
}

#[test]
fn non_uniform_scale_on_arc_is_geometry_error() {
    let mut tp = GcodeIo::new().parse("G2 X10 Y0 I5 J0\n").unwrap();
    let scale = AffineTransform::new(TransformKind::Scale {
        factors: Point3::new(2.0, 1.0, 1.0),
        origin: Point3::default(),
    });
    let error = scale.apply(&mut tp, &Region::Whole).unwrap_err();
    assert!(matches!(error, RosettaError::Geometry { .. }));
}

#[test]
fn uniform_scale_and_units_convert_map_arcs() {
    let mut tp = GcodeIo::new().parse("G2 X10 Y0 I5 J0\n").unwrap();
    let scale = AffineTransform::new(TransformKind::Scale {
        factors: Point3::new(2.0, 2.0, 1.0),
        origin: Point3::default(),
    });
    scale.apply(&mut tp, &Region::Whole).unwrap();
    assert_eq!(tp.segments[0].center, Some(Point2::new(10.0, 0.0)));
    assert!(almost_equal(tp.segments[0].to.x, 20.0, 1e-9));

    let mut tp2 = GcodeIo::new().parse("G21\nG0 X25.4\n").unwrap();
    let convert = AffineTransform::new(TransformKind::UnitsConvert {
        from: Units::Millimeters,
        to: Units::Inches,
    });
    convert.apply(&mut tp2, &Region::Whole).unwrap();
    assert_eq!(tp2.units, Units::Inches);
    assert!(almost_equal(tp2.segments[0].to.x, 1.0, 1e-9));
}

#[test]
fn region_lines_restricts_the_reachable_area() {
    let mut tp = GcodeIo::new().parse("G0 X10 Y0\nG1 X20 Y0\n").unwrap();
    rotate90()
        .apply(&mut tp, &Region::Lines { from: 2, to: 2 })
        .unwrap();
    assert!(almost_equal(tp.segments[0].to.x, 10.0, 1e-9));
    assert!(almost_equal(tp.segments[1].to.x, 0.0, 1e-9));
}

#[test]
fn cycle_words_are_rewritten_in_place() {
    // A rotated G81 must drill at rotated coordinates.
    let mut tp = GcodeIo::new()
        .parse("G90 G54\nG81 X10 Y0 Z-5 F100\nG80\n")
        .unwrap();
    rotate90().apply(&mut tp, &Region::Whole).unwrap();
    let cycle = &tp.codes[1];
    let x = cycle.words.iter().find(|(l, _)| *l == 'X').unwrap().1;
    let y = cycle.words.iter().find(|(l, _)| *l == 'Y').unwrap().1;
    assert!(almost_equal(x, 0.0, 1e-9), "X = {x}");
    assert!(almost_equal(y, 10.0, 1e-9), "Y = {y}");
    // Z is untouched by an XY rotation.
    let z = cycle.words.iter().find(|(l, _)| *l == 'Z').unwrap().1;
    assert!(almost_equal(z, -5.0, 1e-9));
}

#[test]
fn between_marks_region_is_declared_unsupported() {
    let mut tp = Toolpath::default();
    let error = rotate90()
        .apply(
            &mut tp,
            &Region::BetweenMarks {
                start: "A".into(),
                end: "B".into(),
            },
        )
        .unwrap_err();
    assert!(matches!(error, RosettaError::Unsupported { .. }));
}
