//! Applies affine maps to selected toolpath segments and canned-cycle frames.

use super::AffineMap;
use crate::error::RosettaResult;
use crate::ir::toolpath::{Code, Point2, Point3, Toolpath, Units};
use crate::transform::model::{
    region_between_marks_unsupported, Region, Transform, TransformKind, TransformReport,
};

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
            report
                .notes
                .push("program units header rewritten".to_owned());
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
    frame
        .codes
        .iter()
        .any(|c| matches!(c, Code::G(73 | 76 | 81..=89)))
}

/// Rewrite X/Y/Z/I/J words of a cycle frame with `map`.
fn rewrite_cycle_words(frame: &mut crate::ir::toolpath::CodeFrame, map: &AffineMap) {
    // Resolve the current triple with defaults so partial addressing still
    // maps like a point (missing axes are treated as the map of 0.0).
    let get = |letter: char| {
        frame
            .words
            .iter()
            .find(|(l, _)| *l == letter)
            .map(|(_, v)| *v)
    };
    let x = get('X').unwrap_or(0.0);
    let y = get('Y').unwrap_or(0.0);
    let z = get('Z').unwrap_or(0.0);
    let mapped = map.map_point(Point3::new(x, y, z));
    let i = get('I');
    let j = get('J');
    let mapped_vec = i
        .or(j)
        .map(|_| map.map_vector(Point2::new(i.unwrap_or(0.0), j.unwrap_or(0.0))));
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
