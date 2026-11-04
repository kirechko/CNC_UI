//! G-code toolpath parser and emitter (`CORE-T-01/02`) — the concrete
//! [`ToolpathIo`] implementation for the RS-274/GRBL-flavored text format.
//!
//! # Supported subset (Phase 1 / ТЗ №1 §6)
//!
//! * motion codes `G0/G1/G2/G3` (modal — a bare coordinate line continues the
//!   previous mode), `G4` dwell (`P` milliseconds or `X`/`F` seconds),
//! * length system `G20/G21` and distance mode `G90/G91` (modal),
//! * linear address words `X Y Z A B C`, arc offsets `I J` (relative to the
//!   move start, the universal subset), `F` feed, `S` spindle,
//! * comments in both `; line` and `(inline)` syntaxes,
//! * any other G/M codes (cycles `G81..`, coordinate systems `G54..`,
//!   `M6/M30/…`) are preserved *verbatim* as [`CodeFrame`]s so detection
//!   (`TRF-C-04`) and pass-through emission work on the command view without
//!   the parser having to understand every dialect.
//!
//! # Documented limitations
//!
//! * `R`-radius arcs are rejected with an explicit [`RosettaError::Parse`]
//!   instead of silently converting — downstream transforms depend on the
//!   center form (`TRF-C-01` fidelity, principle 4).
//! * Floating point G-codes like `G38.2` do not fit [`Code::G`]'s `u16` and
//!   are kept as [`Code::Other("G38.2")`] verbatim; the capability layer can
//!   still match them by string.
//!
//! # Emission strategy
//!
//! The two IR views are interleaved by their shared `line` numbers: a frame
//! that "owns" segments is re-rendered from the (possibly transformed)
//! geometry; frames without geometry are passed through from their parsed
//! representation; [`CodeFrame::substitute`] lines replace the original text
//! in place, keeping the frame's line number for artifact tracing
//! (`TRF-M-06`). Motion lines always carry an explicit `G90` so emitted text
//! never depends on modal state carried by earlier pass-through lines, which
//! makes `parse → emit → parse` idempotent for the geometry view.

mod emit;
mod parse;

pub use emit::emit_toolpath;
pub use parse::parse_toolpath;

#[cfg(test)]
use crate::error::RosettaError;
use crate::error::RosettaResult;
use crate::io::ToolpathIo;
use crate::ir::toolpath::Toolpath;
#[cfg(test)]
use crate::ir::toolpath::{Code, Point2, Point3, Segment, SegmentKind, Units};

/// The RS-274 text frontend of the Toolpath IR (`CORE-T-02`).
#[derive(Debug, Default, Clone, Copy)]
pub struct GcodeIo;

impl GcodeIo {
    /// The shared stateless instance.
    pub fn new() -> Self {
        GcodeIo
    }
}

impl ToolpathIo for GcodeIo {
    fn parse(&self, gcode: &str) -> RosettaResult<Toolpath> {
        parse_toolpath(gcode)
    }

    fn emit(&self, toolpath: &Toolpath) -> RosettaResult<String> {
        emit_toolpath(toolpath)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small program exercising every parsed construct.
    const SAMPLE: &str =
        "; probe program\nG21\nG90\nG0 X10 Y5\nG1 Z-2 F100\nG2 X20 Y5 I5 J0\nG4 P250\nM6\nM30 ; done\n";

    fn parse_sample() -> Toolpath {
        GcodeIo::new().parse(SAMPLE).unwrap()
    }

    #[test]
    fn parses_two_views_with_modal_state() {
        let tp = parse_sample();
        assert_eq!(tp.units, Units::Millimeters);
        assert_eq!(tp.segment_count(), 4);
        let seg = &tp.segments[0];
        assert_eq!(seg.kind, SegmentKind::Rapid);
        assert_eq!(seg.to, Point3::new(10.0, 5.0, 0.0));
        // Feed is modal: it carries onto the arc even without an F word.
        let arc = &tp.segments[2];
        assert_eq!(arc.kind, SegmentKind::ArcCw);
        assert_eq!(arc.feed, Some(100.0));
        assert_eq!(arc.center, Some(Point2::new(15.0, 5.0)));
        // Dwell keeps its duration in `feed` as milliseconds.
        assert_eq!(tp.segments[3].kind, SegmentKind::Dwell);
        assert_eq!(tp.segments[3].feed, Some(250.0));
        // Command view retains non-motion lines verbatim enough to re-emit.
        assert!(tp.codes.iter().any(|f| f.mentions(&Code::M(6))));
        assert_eq!(tp.header_comments, vec!["probe program".to_owned()]);
    }

    #[test]
    fn incremental_mode_resolves_absolute_points() {
        let tp = GcodeIo::new().parse("G91\nG1 X5 Y2\nG90\nG1 X0\n").unwrap();
        assert_eq!(tp.segments[0].to, Point3::new(5.0, 2.0, 0.0));
        // After G90 the absolute move returns to x = 0, y/z stay as-is.
        assert_eq!(tp.segments[1].to, Point3::new(0.0, 2.0, 0.0));
    }

    #[test]
    fn rejects_r_format_arcs() {
        let error = GcodeIo::new().parse("G2 X10 I5 J0 R3\n").unwrap_err();
        assert!(matches!(error, RosettaError::Parse { line: 1, .. }));
    }

    #[test]
    fn emit_roundtrip_preserves_geometry() {
        let original = parse_sample();
        let text = GcodeIo::new().emit(&original).unwrap();
        let reparsed = GcodeIo::new().parse(&text).unwrap();
        // Line numbers shift in the emitted text (explicit G21 header,
        // dropped blanks), so compare the geometric payload only.
        let project = |s: &Segment| (s.kind, s.from, s.to, s.center, s.feed);
        let before: Vec<_> = original.segments.iter().map(project).collect();
        let after: Vec<_> = reparsed.segments.iter().map(project).collect();
        assert_eq!(after, before, "round trip text:\n{text}");
    }

    #[test]
    fn emit_renders_transformed_coordinates() {
        let mut tp = parse_sample();
        // Simulate a rotate: first rapid destination moves to (−5, 10).
        tp.segments[0].to = Point3::new(-5.0, 10.0, 0.0);
        let text = GcodeIo::new().emit(&tp).unwrap();
        let reparsed = GcodeIo::new().parse(&text).unwrap();
        assert_eq!(reparsed.segments[0].to, Point3::new(-5.0, 10.0, 0.0));
    }

    #[test]
    fn substitute_lines_replace_original_in_place() {
        // TRF-M-06: CycleSubstitute keeps the frame (and its line number)
        // but emits the emulated replacement instead of the original text.
        let mut tp = parse_sample();
        let m6 = tp
            .codes
            .iter()
            .position(|f| f.mentions(&Code::M(6)))
            .unwrap();
        tp.codes[m6].substitute = Some(vec!["(tool_change begin)".into(), "M6".into()]);
        let text = GcodeIo::new().emit(&tp).unwrap();
        assert!(text.contains("(tool_change begin)\nM6\n"), "text: {text}");
        // The original frame still describes its own line for tracing.
        assert_eq!(tp.codes[m6].line, 8);
    }

    #[test]
    fn inline_comments_and_lowercase_tokens_parse() {
        let tp = GcodeIo::new()
            .parse("g1 x2 (rapid-ish) y3 ; feed down\n")
            .unwrap();
        assert_eq!(tp.segments[0].to, Point3::new(2.0, 3.0, 0.0));
        assert_eq!(
            tp.codes[0].comment.as_deref(),
            Some("rapid-ish | feed down")
        );
    }

    #[test]
    fn fractional_gcodes_survive_as_other() {
        // G38.2 (probe) cannot be Code::G(u16); it must round-trip verbatim.
        let tp = GcodeIo::new().parse("G91 G38.2 Z-10 F100\n").unwrap();
        assert!(tp.codes[0].mentions(&Code::Other("G38.2".into())));
        assert_eq!(tp.segments[0].kind, SegmentKind::Linear);
    }

    #[test]
    fn numeric_g_code_over_u16_range_is_a_line_parse_error() {
        let error = GcodeIo::new().parse("G21\nG76567\n").unwrap_err();
        assert!(matches!(error, RosettaError::Parse { line: 2, .. }));
        assert!(error.to_string().contains("G76567"));
    }

    #[test]
    fn largest_u16_g_code_number_is_preserved_for_capability_scan() {
        let tp = GcodeIo::new().parse("G65535\n").unwrap();
        assert!(tp.codes[0].mentions(&Code::G(u16::MAX)));
    }

    #[test]
    fn malformed_word_reports_line() {
        let error = GcodeIo::new().parse("G1 Xfoo\n").unwrap_err();
        assert!(matches!(error, RosettaError::Parse { line: 1, .. }));
    }
}
