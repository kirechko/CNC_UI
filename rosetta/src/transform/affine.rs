//! Affine transforms (ТЗ №3 §4А, Phase A): `Rotate`, `Translate`, `Scale`,
//! `Mirror`, `UnitsConvert`.
//!
//! All five reduce to one 2D affine matrix for the XY plane plus a scalar
//! rule for Z, which is exactly what makes them *affine* — no new geometry,
//! only recomputed coordinates:
//!
//! ```text
//! | x' |   | a  b | | x |   | e |
//! | y' | = | c  d | | y | + | f |
//! z' = z * z_factor + z_delta
//! ```
//!
//! * Rotation is the only kind with a nonzero `b`/`c` beyond scaling;
//!   `det = a*d − b*c`; `det < 0` means a reflection, which flips arc
//!   direction (a mirrored clockwise circle is counter-clockwise).
//! * Arcs survive an affine map only when it is a *similarity* (uniform
//!   scale + rotation + reflection + translation); a non-uniform `Scale`
//!   turns circles into ellipses, and the engine refuses with a
//!   [`RosettaError::Geometry`] instead of emitting fake arcs (principle 4).
//! * Cycle frames (`G81`…) have no segments, so their `X/Y/Z` address words
//!   are rewritten directly by the same matrix; arc offsets `I/J` are
//!   *vectors* and use the linear part without the translation column.

use crate::error::{RosettaError, RosettaResult};
use crate::ir::toolpath::{Code, Point2, Point3, Segment, SegmentKind, Toolpath, Units};
use crate::transform::model::{
    region_between_marks_unsupported, MirrorAxes, Region, Transform, TransformKind,
    TransformReport,
};
use crate::util::almost_equal;

/// A 2D similarity/affine map plus the scalar Z rule, shared by all kinds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineMap {
    /// Linear part rows: `x' = a*x + b*y + e`, `y' = c*x + d*y + f`.
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
    /// Z scales by `z_factor` then shifts by `z_delta`.
    pub z_factor: f64,
    pub z_delta: f64,
}

impl AffineMap {
    /// The identity map.
    pub fn identity() -> Self {
        AffineMap { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0, z_factor: 1.0, z_delta: 0.0 }
    }

    /// Pure translation.
    pub fn translation(dx: f64, dy: f64, dz: f64) -> Self {
        AffineMap { e: dx, f: dy, z_delta: dz, ..AffineMap::identity() }
    }

    /// Uniform scale around the origin.
    pub fn uniform_scale(s: f64) -> Self {
        AffineMap { a: s, d: s, z_factor: s, ..AffineMap::identity() }
    }

    /// Determinant of the XY linear part; negative ⇒ reflection.
    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// Whether the XY part is a similarity (rotation+reflection+uniform
    /// scale): maps circles to circles. The tolerance is relative because
    /// `UnitsConvert` factors like 1/25.4 are small.
    pub fn is_similarity(&self, eps: f64) -> bool {
        let scale_sq = (self.a * self.a + self.c * self.c).sqrt();
        almost_equal(self.a, self.d, eps * scale_sq.max(1.0))
            && almost_equal(self.b, -self.c, eps * scale_sq.max(1.0))
            || almost_equal(self.a, -self.d, eps * scale_sq.max(1.0))
                && almost_equal(self.b, self.c, eps * scale_sq.max(1.0))
    }

    /// The uniform XY scale factor of a similarity (`None` otherwise).
    pub fn uniform_factor(&self) -> Option<f64> {
        self.is_similarity(1e-9).then(|| (self.a * self.a + self.c * self.c).sqrt())
    }

    /// Map a planar point.
    pub fn map_xy(&self, p: Point2) -> Point2 {
        Point2::new(self.a * p.x + self.b * p.y + self.e, self.c * p.x + self.d * p.y + self.f)
    }

    /// Map a space point including the scalar Z rule.
    pub fn map_point(&self, p: Point3) -> Point3 {
        let xy = self.map_xy(p.xy());
        Point3 { x: xy.x, y: xy.y, z: p.z * self.z_factor + self.z_delta, ..p }
    }

    /// Map a *vector* (no translation column) — arc offsets `I/J`.
    pub fn map_vector(&self, p: Point2) -> Point2 {
        Point2::new(self.a * p.x + self.b * p.y, self.c * p.x + self.d * p.y)
    }

    /// Build the map for an affine [`TransformKind`].
    pub fn from_kind(kind: &TransformKind) -> RosettaResult<Self> {
        Ok(match kind {
            TransformKind::Rotate { angle_deg, center } => {
                // Standard rotation about `center`: translate origin to (0,0),
                // rotate CCW for positive angle, translate back.
                let rad = angle_deg.to_radians();
                let (cos, sin) = (rad.cos(), rad.sin());
                AffineMap {
                    a: cos,
                    b: -sin,
                    c: sin,
                    d: cos,
                    e: center.x - cos * center.x + sin * center.y,
                    f: center.y - sin * center.x - cos * center.y,
                    z_factor: 1.0,
                    z_delta: 0.0,
                }
            }
            TransformKind::Translate { delta } => {
                AffineMap::translation(delta.x, delta.y, delta.z)
            }
            TransformKind::Scale { factors, origin } => {
                // x' = ox + sx*(x-ox) ⇒ a = sx, e = ox*(1-sx); same per axis.
                AffineMap {
                    a: factors.x,
                    b: 0.0,
                    c: 0.0,
                    d: factors.y,
                    e: origin.x * (1.0 - factors.x),
                    f: origin.y * (1.0 - factors.y),
                    z_factor: factors.z,
                    z_delta: origin.z * (1.0 - factors.z),
                }
            }
            TransformKind::Mirror { axes } => match axes {
                MirrorAxes::X => AffineMap { a: -1.0, ..AffineMap::identity() },
                MirrorAxes::Y => AffineMap { d: -1.0, ..AffineMap::identity() },
                // Mirroring both axes = 180° rotation, determinant +1: the
                // arc direction is *preserved*, a classic gotcha covered by
                // the det<0 rule below.
                MirrorAxes::Xy => AffineMap { a: -1.0, d: -1.0, ..AffineMap::identity() },
            },
            TransformKind::UnitsConvert { from, to } => {
                AffineMap::uniform_scale(from.factor_to(*to))
            }
            other => {
                return Err(RosettaError::Validation {
                    reason: format!("{} is not an affine transform", other.name()),
                })
            }
        })
    }

    /// Rewrite one segment; `Err` for arc under non-similarity maps.
    pub fn map_segment(&self, segment: &Segment) -> RosettaResult<(Segment, bool)> {
        let mut out = segment.clone();
        out.from = self.map_point(segment.from);
        out.to = self.map_point(segment.to);
        let mut flipped = false;
        if segment.kind.is_arc() {
            let scale = self.uniform_factor().ok_or_else(|| RosettaError::Geometry {
                reason: format!(
                    "segment {}: non-uniform affine map would turn the arc into an ellipse",
                    segment.id
                ),
            })?;
            let center = segment.center.ok_or_else(|| RosettaError::Geometry {
                reason: format!("segment {}: arc without center", segment.id),
            })?;
            out.center = Some(self.map_xy(center));
            if self.det() < 0.0 {
                out.kind = match segment.kind {
                    SegmentKind::ArcCw => SegmentKind::ArcCcw,
                    SegmentKind::ArcCcw => SegmentKind::ArcCw,
                    other => other,
                };
                flipped = true;
            }
            let _ = scale; // radius change is implicit in the mapped geometry
        }
        // Feed rates rescale under a pure uniform scale similarity
        // (`applies_to_feed`): that is what `UnitsConvert` needs, and it is
        // consistent for an isotropic `Scale`. Dwells are exempt — their
        // `feed` field stores milliseconds, a duration, not a distance rate.
        if self.applies_to_feed() && segment.kind != SegmentKind::Dwell {
            if let Some(feed) = segment.feed {
                out.feed = Some(feed * self.a);
            }
        }
        Ok((out, flipped))
    }

    /// Whether this map is a *pure uniform scale* (`a == d == z_factor`, no
    /// rotation, reflection, shear or translation) whose factor differs from
    /// 1 — the only case in which programmed feed rates must be rescaled
    /// (TRF §4А note; dwells are still exempt inside `map_segment`).
    pub fn applies_to_feed(&self) -> bool {
        self.a == self.d
            && self.z_factor == self.a
            && self.a != 1.0
            && self.b == 0.0
            && self.c == 0.0
            && self.e == 0.0
            && self.f == 0.0
            && self.z_delta == 0.0
    }
}

/// The affine executor over an IR region (`TRF-C-01/06`).
#[derive(Debug, Clone)]
pub struct AffineTransform {
    /// The operation to perform; must be from the affine family.
    pub kind: TransformKind,
}

impl AffineTransform {
    /// Wrap an affine kind; non-affine kinds fail at [`Transform::apply`].
    pub fn new(kind: TransformKind) -> Self {
        AffineTransform { kind }
    }
}

impl Transform for AffineTransform {
    fn name(&self) -> &'static str {
        self.kind.name()
    }

    /// Apply the affine map to every segment and cycle frame inside `region`.
    fn apply(&self, toolpath: &mut Toolpath, region: &Region) -> RosettaResult<TransformReport> {
        let map = AffineMap::from_kind(&self.kind)?;
        // `if let`, not `matches!`: the marker names are needed for the error.
        if let Region::BetweenMarks { start, end } = region {
            return Err(region_between_marks_unsupported(start, end));
        }
        let mut report = TransformReport {
            affected_segments: 0,
            affected_lines: Vec::new(),
            notes: Vec::new(),
        };
        for segment in &mut toolpath.segments {
            if !region_has_segment(region, segment.id, segment.line) {
                continue;
            }
            let (mapped, flipped) = map.map_segment(segment)?;
            if flipped {
                report.notes.push(format!(
                    "arc segment {} direction flipped by reflection",
                    segment.id
                ));
            }
            *segment = mapped;
            report.affected_segments += 1;
            if !report.affected_lines.contains(&segment.line) {
                report.affected_lines.push(segment.line);
            }
        }
        // Cycle frames (G73/81..89) carry coordinates as address words with
        // no segment view; rewrite them in place so a rotated program drills
        // rotated holes (TRF-C-06 tracing: line numbers are untouched).
        for frame in &mut toolpath.codes {
            if !region_selects_frame(region, frame.line) || !frame_has_cycle(frame) {
                continue;
            }
            rewrite_cycle_words(frame, &map);
            if !report.affected_lines.contains(&frame.line) {
                report.affected_lines.push(frame.line);
            }
        }
        if self.kind_units_convert() {
            toolpath.units = units_target(&self.kind);
            report.notes.push("program units header rewritten".to_owned());
        }
        Ok(report)
    }
}

impl AffineTransform {
    fn kind_units_convert(&self) -> bool {
        matches!(self.kind, TransformKind::UnitsConvert { .. })
    }
}

/// Target units of a [`TransformKind::UnitsConvert`].
fn units_target(kind: &TransformKind) -> Units {
    match kind {
        TransformKind::UnitsConvert { to, .. } => *to,
        _ => Units::Millimeters,
    }
}

/// Whether `region` selects the segment with `id` (at `line`).
fn region_has_segment(region: &Region, id: crate::ir::toolpath::SegmentId, line: usize) -> bool {
    match region {
        Region::Whole => true,
        Region::Lines { from, to } => (*from..=*to).contains(&line),
        Region::BetweenMarks { .. } => false, // validated earlier
        Region::Segments(ids) => ids.contains(&id),
    }
}

/// Whether `region` selects a *frame* at source `line`. A `Segments` region
/// cannot address cycle frames at all — those lines produce no segment ids —
/// so it is skipped there (documented limitation of the region model).
fn region_selects_frame(region: &Region, line: usize) -> bool {
    match region {
        Region::Whole => true,
        Region::Lines { from, to } => (*from..=*to).contains(&line),
        Region::BetweenMarks { .. } => false, // validated earlier
        Region::Segments(_) => false,
    }
}

/// True when the frame is a canned-cycle line (its coordinates live only in
/// words and must be rewritten by an affine map).
fn frame_has_cycle(frame: &crate::ir::toolpath::CodeFrame) -> bool {
    frame.codes.iter().any(|c| matches!(c, Code::G(73 | 76 | 81..=89)))
}

/// Rewrite X/Y/Z/I/J words of a cycle frame with `map`.
fn rewrite_cycle_words(frame: &mut crate::ir::toolpath::CodeFrame, map: &AffineMap) {
    // Resolve the current triple with defaults so partial addressing still
    // maps like a point (missing axes are treated as the map of 0.0).
    let get = |letter: char| frame.words.iter().find(|(l, _)| *l == letter).map(|(_, v)| *v);
    let x = get('X').unwrap_or(0.0);
    let y = get('Y').unwrap_or(0.0);
    let z = get('Z').unwrap_or(0.0);
    let mapped = map.map_point(Point3::new(x, y, z));
    let i = get('I');
    let j = get('J');
    let mapped_vec = i.or(j).map(|_| {
        map.map_vector(Point2::new(i.unwrap_or(0.0), j.unwrap_or(0.0)))
    });
    for (letter, value) in &mut frame.words {
        match letter {
            'X' => *value = mapped.x,
            'Y' => *value = mapped.y,
            'Z' => *value = mapped.z,
            'I' => {
                if let Some(v) = &mapped_vec {
                    *value = v.x;
                }
            }
            'J' => {
                if let Some(v) = &mapped_vec {
                    *value = v.y;
                }
            }
            // Feed words inside cycle frames follow the same rule as segment
            // feeds: only a pure uniform similarity rescales them.
            'F' => {
                if map.applies_to_feed() {
                    *value *= map.a;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::gcode::GcodeIo;
    use crate::io::ToolpathIo;

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
        assert!(almost_equal(p.x, 0.0, 1e-9) && almost_equal(p.y, 10.0, 1e-9), "{p:?}");
        // Rotation around a nonzero center: (11,1) about (1,1) → (1, 11).
        let map = AffineMap::from_kind(&TransformKind::Rotate {
            angle_deg: 90.0,
            center: Point2::new(1.0, 1.0),
        })
        .unwrap();
        let p = map.map_xy(Point2::new(11.0, 1.0));
        assert!(almost_equal(p.x, 1.0, 1e-9) && almost_equal(p.y, 11.0, 1e-9), "{p:?}");
    }

    #[test]
    fn rotate_toolpath_segments_and_reports_lines() {
        let mut tp = GcodeIo::new()
            .parse("G0 X10 Y0\nG1 X10 Y5 F100\n")
            .unwrap();
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
        let mirror_x = AffineTransform::new(TransformKind::Mirror { axes: MirrorAxes::X });
        let report = mirror_x.apply(&mut tp, &Region::Whole).unwrap();
        assert_eq!(tp.segments[0].kind, SegmentKind::ArcCcw);
        assert!(report.notes.iter().any(|n| n.contains("flipped")));

        let mut tp2 = GcodeIo::new().parse("G2 X10 Y0 I5 J0\n").unwrap();
        let mirror_xy = AffineTransform::new(TransformKind::Mirror { axes: MirrorAxes::Xy });
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
                &Region::BetweenMarks { start: "A".into(), end: "B".into() },
            )
            .unwrap_err();
        assert!(matches!(error, RosettaError::Unsupported { .. }));
    }
}
