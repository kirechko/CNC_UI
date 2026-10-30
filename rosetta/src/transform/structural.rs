//! Structural transforms (ТЗ №3 §4В, Phases B/C): `FeedSpeedOverride`,
//! `CoordSystemRemap`, `CycleSubstitute`, `StripUnsupported` — plus the
//! explicit stubs for the deferred geometric family (§4Б, Phase D).
//!
//! Structural operations rewrite the *organization* of the program, not its
//! XY geometry, so they mostly edit [`CodeFrame`]s and segment metadata while
//! line tracing (`TRF-C-06`) stays untouched. `CycleSubstitute` is the
//! contract bridge: it stores its emulating lines into
//! [`CodeFrame::substitute`], and the G-code emitter replaces the original
//! line with them while the frame keeps the original for the artifact trace
//! (`TRF-M-06`).

use crate::error::{RosettaError, RosettaResult};
use crate::ir::toolpath::{Code, SegmentKind, Toolpath};
use crate::transform::model::{
    region_between_marks_unsupported, CoordSystem, MacroExpander, Region, Transform,
    TransformKind, TransformReport,
};

/// Executor for the structural family. Not `Debug` because it holds a trait
/// object reference; the kind itself is printable via `Transform::name`.
#[derive(Clone)]
pub struct StructuralTransform<'a> {
    /// The operation; must belong to [`TransformFamily::Structural`].
    pub kind: TransformKind,
    /// Host bridge for [`TransformKind::CycleSubstitute`] (`TRF-M-01`);
    /// `None` makes substitution fail instead of guessing lines.
    pub expander: Option<&'a dyn MacroExpander>,
}

impl<'a> StructuralTransform<'a> {
    /// Wrap a structural kind with an optional macro-expander bridge.
    pub fn new(kind: TransformKind, expander: Option<&'a dyn MacroExpander>) -> Self {
        StructuralTransform { kind, expander }
    }
}

impl Transform for StructuralTransform<'_> {
    fn name(&self) -> &'static str {
        self.kind.name()
    }

    fn apply(&self, toolpath: &mut Toolpath, region: &Region) -> RosettaResult<TransformReport> {
        // `if let` binds the marker names for the error payload.
        if let Region::BetweenMarks { start, end } = region {
            return Err(region_between_marks_unsupported(start, end));
        }
        match &self.kind {
            TransformKind::FeedSpeedOverride { feed_factor, speed_factor } => {
                feed_speed_override(toolpath, region, *feed_factor, *speed_factor)
            }
            TransformKind::CoordSystemRemap { from, to } => {
                coord_system_remap(toolpath, region, *from, *to)
            }
            TransformKind::CycleSubstitute { from, to } => {
                self.cycle_substitute(toolpath, region, *from, to)
            }
            TransformKind::StripUnsupported { codes } => strip_unsupported(toolpath, region, codes),
            other => Err(RosettaError::Validation {
                reason: format!("{} is not a structural transform", other.name()),
            }),
        }
    }
}

impl StructuralTransform<'_> {
    /// TRF-M-05/06: ask the host bridge for emulating lines and store them
    /// on the cycle frame; the original frame stays for tracing.
    fn cycle_substitute(
        &self,
        toolpath: &mut Toolpath,
        region: &Region,
        from: crate::transform::model::CycleCode,
        to: &str,
    ) -> RosettaResult<TransformReport> {
        let expander = self.expander.ok_or_else(|| RosettaError::NotFound {
            kind: "macro expander bridge".into(),
            id: to.to_owned(),
        })?;
        let mut report =
            TransformReport { affected_segments: 0, affected_lines: Vec::new(), notes: Vec::new() };
        for frame in &mut toolpath.codes {
            if !selects_line(region, frame.line) || !frame.mentions(&from.code()) {
                continue;
            }
            // TRF-M-05: idempotency — a frame already substituted is skipped
            // so running the step twice changes nothing.
            if frame.substitute.is_some() {
                report.notes.push(format!("line {} already substituted", frame.line));
                continue;
            }
            let lines = expander.expand(to, from, frame)?;
            if lines.is_empty() {
                return Err(RosettaError::Runtime {
                    reason: format!("expander produced no lines for {from:?} -> macro \"{to}\""),
                });
            }
            frame.substitute = Some(lines);
            report.affected_lines.push(frame.line);
        }
        report.notes.push(format!("substituted {} cycle line(s)", report.affected_lines.len()));
        Ok(report)
    }
}

/// Multiply feed-bearing values by `feed_factor` and spindle `S` words by
/// `speed_factor`. Dwells keep their duration; feedless moves stay feedless.
fn feed_speed_override(
    toolpath: &mut Toolpath,
    region: &Region,
    feed_factor: f64,
    speed_factor: f64,
) -> RosettaResult<TransformReport> {
    if feed_factor <= 0.0 || speed_factor <= 0.0 || !feed_factor.is_finite() || !speed_factor.is_finite() {
        return Err(RosettaError::Validation {
            reason: "feed/speed factors must be positive finite numbers".into(),
        });
    }
    let mut report =
        TransformReport { affected_segments: 0, affected_lines: Vec::new(), notes: Vec::new() };
    for segment in &mut toolpath.segments {
        if !selects_segment(region, segment.id, segment.line) {
            continue;
        }
        if let Some(feed) = segment.feed {
            if segment.kind != SegmentKind::Dwell {
                segment.feed = Some(feed * feed_factor);
                touch(&mut report, segment.line);
            }
        }
    }
    for frame in &mut toolpath.codes {
        if !selects_line(region, frame.line) {
            continue;
        }
        let mut changed = false;
        for (letter, value) in &mut frame.words {
            match letter {
                // `F` inside a `G4` frame is a dwell seconds word, not feed.
                'F' if !frame.codes.contains(&Code::G(4)) => {
                    *value *= feed_factor;
                    changed = true;
                }
                'S' => {
                    *value *= speed_factor;
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            touch(&mut report, frame.line);
        }
    }
    Ok(report)
}

/// Replace one work-coordinate code with another in every frame word list.
/// Offsets themselves are controller-side data (`TRF-C-01`: geometry is not
/// touched — the machine applies a different origin at run time).
fn coord_system_remap(
    toolpath: &mut Toolpath,
    region: &Region,
    from: CoordSystem,
    to: CoordSystem,
) -> RosettaResult<TransformReport> {
    if from == to {
        return Ok(TransformReport {
            affected_segments: 0,
            affected_lines: Vec::new(),
            notes: vec!["remapped to the same system: no-op".into()],
        });
    }
    let mut report =
        TransformReport { affected_segments: 0, affected_lines: Vec::new(), notes: Vec::new() };
    for frame in &mut toolpath.codes {
        if !selects_line(region, frame.line) {
            continue;
        }
        let mut changed = false;
        for code in &mut frame.codes {
            if *code == from.code() {
                *code = to.code();
                changed = true;
            }
        }
        if changed {
            touch(&mut report, frame.line);
        }
    }
    Ok(report)
}

/// Delete selected codes from frames. Motion segments owned by a *pure*
/// cycle line (the frame carries a G73/G76/G81..G89 code) are dropped with
/// the code, because keeping the geometry while removing the cycle header
/// would silently change its meaning.
fn strip_unsupported(
    toolpath: &mut Toolpath,
    region: &Region,
    codes: &[Code],
) -> RosettaResult<TransformReport> {
    let mut report =
        TransformReport { affected_segments: 0, affected_lines: Vec::new(), notes: Vec::new() };
    let mut dropped_lines: Vec<usize> = Vec::new();
    for frame in &mut toolpath.codes {
        if !selects_line(region, frame.line) {
            continue;
        }
        let before = frame.codes.len();
        // Remember whether a *cycle* code is among the removed ones before
        // mutating: only then do the move lines below lose their meaning.
        let removed_cycle = frame.codes.iter().any(|code| is_cycle(code) && codes.contains(code));
        frame.codes.retain(|code| !codes.contains(code));
        if frame.codes.len() != before {
            touch(&mut report, frame.line);
        }
        if removed_cycle {
            dropped_lines.push(frame.line);
        }
    }
    if !dropped_lines.is_empty() {
        let lines = &dropped_lines;
        let removed = toolpath.segments.len();
        toolpath.segments.retain(|segment| !lines.contains(&segment.line));
        report.affected_segments = removed - toolpath.segments.len();
        report.notes.push(format!(
            "dropped {} segment(s) on stripped cycle lines {:?}",
            report.affected_segments, dropped_lines
        ));
    }
    Ok(report)
}

/// True for canned-cycle codes.
fn is_cycle(code: &Code) -> bool {
    matches!(code, Code::G(73 | 76 | 81..=89))
}

/// Record `line` in the report exactly once, keeping first-seen order.
fn touch(report: &mut TransformReport, line: usize) {
    if !report.affected_lines.contains(&line) {
        report.affected_lines.push(line);
    }
}

/// Region predicate over source lines.
fn selects_line(region: &Region, line: usize) -> bool {
    match region {
        Region::Whole => true,
        Region::Lines { from, to } => (*from..=*to).contains(&line),
        Region::BetweenMarks { .. } => false,
        Region::Segments(_) => true,
    }
}

/// Region predicate over segments.
fn selects_segment(region: &Region, id: crate::ir::toolpath::SegmentId, line: usize) -> bool {
    match region {
        Region::Whole => true,
        Region::Lines { from, to } => (*from..=*to).contains(&line),
        Region::BetweenMarks { .. } => false,
        Region::Segments(ids) => ids.contains(&id),
    }
}

/// Explicit placeholder for the deferred geometric family (§4Б, Phase D):
/// `CutterCompBake`, `LeadInOut`, `CornerRound` fail with `Unsupported`
/// naming the phase rather than producing approximate geometry (principle 4).
#[derive(Debug, Clone)]
pub struct GeometricStub {
    /// The requested kind; kept so `name()` and the error say what was asked.
    pub kind: TransformKind,
}

impl GeometricStub {
    /// Wrap a geometric kind.
    pub fn new(kind: TransformKind) -> Self {
        GeometricStub { kind }
    }
}

impl Transform for GeometricStub {
    fn name(&self) -> &'static str {
        self.kind.name()
    }

    fn apply(&self, _toolpath: &mut Toolpath, _region: &Region) -> RosettaResult<TransformReport> {
        Err(RosettaError::Unsupported {
            what: format!("geometric transform {}", self.kind.name()),
            phase: "D".into(),
        })
    }
}

/// Build the executor for any [`TransformKind`] (`registry` glue, TRF-M-03):
/// affine family → [`crate::transform::affine::AffineTransform`], structural
/// → [`StructuralTransform`], geometric → [`GeometricStub`].
pub fn executor<'a>(
    kind: &TransformKind,
    expander: Option<&'a dyn MacroExpander>,
) -> Box<dyn Transform + 'a> {
    use crate::transform::affine::AffineTransform;
    match kind.family() {
        crate::transform::model::TransformFamily::Affine => Box::new(AffineTransform::new(kind.clone())),
        crate::transform::model::TransformFamily::Structural => {
            Box::new(StructuralTransform::new(kind.clone(), expander))
        }
        crate::transform::model::TransformFamily::Geometric => {
            Box::new(GeometricStub::new(kind.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::gcode::GcodeIo;
    use crate::io::ToolpathIo;
    use crate::transform::model::CycleCode;

    /// Expander returning canned lines per (macro, cycle) pair.
    struct CannedExpander;
    impl MacroExpander for CannedExpander {
        fn expand(
            &self,
            macro_id: &str,
            cycle: CycleCode,
            frame: &crate::ir::toolpath::CodeFrame,
        ) -> RosettaResult<Vec<String>> {
            let z = frame.words.iter().find(|(l, _)| *l == 'Z').map(|(_, v)| *v).unwrap_or(0.0);
            Ok(vec![format!("({macro_id} emulate {cycle:?})"), format!("G1 Z{z}")])
        }
    }

    #[test]
    fn feed_override_scales_feeds_but_not_dwell_or_spindle() {
        let mut tp = GcodeIo::new()
            .parse("G1 X1 F200\nG4 P500\nM3 S10000\n")
            .unwrap();
        let tf = StructuralTransform::new(
            TransformKind::FeedSpeedOverride { feed_factor: 0.5, speed_factor: 2.0 },
            None,
        );
        tf.apply(&mut tp, &Region::Whole).unwrap();
        assert_eq!(tp.segments[0].feed, Some(100.0));
        assert_eq!(tp.segments[1].feed, Some(500.0)); // dwell untouched
        let spindle = &tp.codes[2];
        assert_eq!(spindle.words.iter().find(|(l, _)| *l == 'S').unwrap().1, 20000.0);
    }

    #[test]
    fn coord_remap_rewrites_g54_to_g56() {
        let mut tp = GcodeIo::new().parse("G54 G0 X1\nG54 G1 Z-1\n").unwrap();
        let tf = StructuralTransform::new(
            TransformKind::CoordSystemRemap { from: CoordSystem::G54, to: CoordSystem::G56 },
            None,
        );
        let report = tf.apply(&mut tp, &Region::Whole).unwrap();
        assert_eq!(report.affected_lines, vec![1, 2]);
        assert!(tp.codes.iter().all(|f| !f.mentions(&CoordSystem::G54.code())));
        assert!(tp.codes.iter().all(|f| f.mentions(&CoordSystem::G56.code())));
    }

    #[test]
    fn cycle_substitute_stores_lines_and_keeps_original_frame() {
        // TRF-M-06: after substitution the frame still mentions G83 and keeps
        // its line, while `substitute` carries the emulating program text.
        let mut tp = GcodeIo::new().parse("G83 Z-10 Q2 F100\n").unwrap();
        let tf = StructuralTransform::new(
            TransformKind::CycleSubstitute {
                from: CycleCode::G83,
                to: "g83_peck_drill".into(),
            },
            Some(&CannedExpander),
        );
        let report = tf.apply(&mut tp, &Region::Whole).unwrap();
        assert_eq!(report.affected_lines, vec![1]);
        let frame = &tp.codes[0];
        assert!(frame.mentions(&Code::G(83)));
        assert_eq!(frame.substitute.as_deref().unwrap()[1], "G1 Z-10");
        // Idempotency (TRF-M-05): applying again changes nothing.
        let report = tf.apply(&mut tp, &Region::Whole).unwrap();
        assert!(report.affected_lines.is_empty());
    }

    #[test]
    fn cycle_substitute_without_bridge_fails() {
        let mut tp = GcodeIo::new().parse("G83 Z-10\n").unwrap();
        let tf = StructuralTransform::new(
            TransformKind::CycleSubstitute { from: CycleCode::G83, to: "x".into() },
            None,
        );
        let error = tf.apply(&mut tp, &Region::Whole).unwrap_err();
        assert!(matches!(error, RosettaError::NotFound { .. }));
    }

    #[test]
    fn strip_removes_mcodes_and_keeps_geometry() {
        let mut tp = GcodeIo::new().parse("M6\nG0 X5\n").unwrap();
        let tf = StructuralTransform::new(
            TransformKind::StripUnsupported { codes: vec![Code::M(6)] },
            None,
        );
        let report = tf.apply(&mut tp, &Region::Whole).unwrap();
        assert_eq!(report.affected_lines, vec![1]);
        assert!(tp.codes[0].codes.is_empty());
        assert_eq!(tp.segment_count(), 1);
    }

    #[test]
    fn geometric_family_is_deferred_to_phase_d() {
        let mut tp = Toolpath::default();
        let tf = GeometricStub::new(TransformKind::CutterCompBake {
            side: crate::transform::model::CompSide::Left,
            tool_radius: 1.5,
            lead: None,
        });
        let error = tf.apply(&mut tp, &Region::Whole).unwrap_err();
        assert!(matches!(error, RosettaError::Unsupported { ref phase, .. } if phase == "D"));
    }

    #[test]
    fn executor_dispatches_by_family() {
        let rotate = TransformKind::Rotate {
            angle_deg: 90.0,
            center: crate::ir::toolpath::Point2::new(0.0, 0.0),
        };
        assert_eq!(executor(&rotate, None).name(), "Rotate");
        let bake = TransformKind::CornerRound { radius: 2.0 };
        assert_eq!(executor(&bake, None).name(), "CornerRound");
    }
}
