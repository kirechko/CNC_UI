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

use std::collections::HashMap;

use crate::error::{RosettaError, RosettaResult};
use crate::io::ToolpathIo;
use crate::ir::toolpath::{
    Code, CodeFrame, Point2, Point3, Segment, SegmentId, SegmentKind, Toolpath, Units,
};
use crate::util::fmt_num;

/// Axis words that define a motion target, in canonical emission order. The
/// `usize` is the index into [`PointSlot`] helpers below.
const AXIS_WORDS: [(char, usize); 6] = [('X', 0), ('Y', 1), ('Z', 2), ('A', 3), ('B', 4), ('C', 5)];

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

/// Internal mutable state of one parse pass (modal G-code tracking).
struct ParserState {
    /// `G90`/`G91`: whether coordinates are absolute (default true).
    absolute: bool,
    /// Current modal motion mode; `None` until the first motion line.
    mode: Option<SegmentKind>,
    /// Tool center point, absolute, updated by every motion line.
    pos: Point3,
    /// Modal feed rate.
    feed: Option<f64>,
    /// Next segment id.
    next_id: SegmentId,
}

/// Parse `source` into the two-view [`Toolpath`] IR.
pub fn parse_toolpath(source: &str) -> RosettaResult<Toolpath> {
    let mut state = ParserState {
        absolute: true,
        mode: None,
        pos: Point3::default(),
        feed: None,
        next_id: 0,
    };
    // Units are modal as well but belong to the result, so they live in a
    // local that is written into `Toolpath::units` at the very end.
    let mut units = Units::Millimeters;
    let mut toolpath = Toolpath {
        units,
        segments: Vec::new(),
        codes: Vec::new(),
        header_comments: Vec::new(),
    };

    for (index, raw_line) in source.lines().enumerate() {
        let line = index + 1;
        if raw_line.trim().is_empty() {
            // Blank lines are dropped: they carry no command and would only
            // shift line numbers in reports.
            continue;
        }
        let (body, comment) = split_comments(raw_line);
        let mut codes: Vec<Code> = Vec::new();
        // Address words in first-occurrence order; a letter repeated on one
        // line keeps its slot but takes the last value (controller behavior).
        let mut words: Vec<(char, f64)> = Vec::new();
        let mut seen: HashMap<char, f64> = HashMap::new();
        for token in body.split_whitespace() {
            let upper = token.to_ascii_uppercase();
            let mut chars = upper.chars();
            let letter = match chars.next() {
                Some(c) => c,
                None => continue,
            };
            let value_text: String = chars.collect();
            let value: f64 = value_text.parse().map_err(|_| RosettaError::Parse {
                line,
                reason: format!("malformed word \"{token}\""),
            })?;
            match letter {
                'G' | 'M' => {
                    codes.push(
                        Code::parse(&upper)
                            .unwrap_or_else(|| Code::Other(strip_zero_padding(&upper))),
                    );
                }
                other => {
                    match seen.get(&other) {
                        Some(_) => {
                            // Update the existing slot in place.
                            if let Some(slot) = words.iter_mut().find(|(l, _)| *l == other) {
                                slot.1 = value;
                            }
                        }
                        None => {
                            words.push((other, value));
                        }
                    }
                    seen.insert(other, value);
                }
            }
        }

        let mut frame = CodeFrame {
            line,
            codes: codes.clone(),
            words: words.clone(),
            raw: body.trim().to_owned(),
            // `split_comments` already returns an owned optional comment.
            comment,
            substitute: None,
        };

        // ---- modal bookkeeping ------------------------------------------------
        if frame.codes.contains(&Code::G(20)) {
            units = Units::Inches;
        }
        if frame.codes.contains(&Code::G(21)) {
            units = Units::Millimeters;
        }
        if frame.codes.contains(&Code::G(90)) {
            state.absolute = true;
        }
        if frame.codes.contains(&Code::G(91)) {
            state.absolute = false;
        }

        // ---- dwell: G4 P.. (ms) or X../F.. (seconds) --------------------------
        if frame.codes.contains(&Code::G(4)) {
            let ms = match (seen.get(&'P'), seen.get(&'X'), seen.get(&'F')) {
                (Some(ms), _, _) => *ms,
                (_, Some(seconds), _) => seconds * 1000.0,
                (_, _, Some(seconds)) => seconds * 1000.0,
                _ => {
                    return Err(RosettaError::Parse {
                        line,
                        reason: "G4 requires P (ms), X (s) or F (s)".to_owned(),
                    })
                }
            };
            toolpath.segments.push(Segment {
                id: state.next_id,
                kind: SegmentKind::Dwell,
                from: state.pos,
                to: state.pos,
                center: None,
                feed: Some(ms),
                line,
            });
            state.next_id += 1;
            toolpath.codes.push(frame);
            continue;
        }

        // ---- motion -------------------------------------------------------------
        // Modal code updated by any G0..G3 on this line (last one wins).
        for code in &frame.codes {
            if let Code::G(n @ 0..=3) = code {
                state.mode = Some(match n {
                    0 => SegmentKind::Rapid,
                    1 => SegmentKind::Linear,
                    2 => SegmentKind::ArcCw,
                    _ => SegmentKind::ArcCcw,
                });
            }
        }
        let has_axes = AXIS_WORDS.iter().any(|(letter, _)| seen.contains_key(letter));
        if has_axes {
            let mode = state.mode.unwrap_or(SegmentKind::Linear);
            let center = if mode.is_arc() {
                if seen.contains_key(&'R') {
                    return Err(RosettaError::Parse {
                        line,
                        reason: "R-format arcs are not supported; rewrite to center I/J form"
                            .to_owned(),
                    });
                }
                let i = *seen.get(&'I').unwrap_or(&0.0);
                let j = *seen.get(&'J').unwrap_or(&0.0);
                if i == 0.0 && j == 0.0 {
                    return Err(RosettaError::Parse {
                        line,
                        reason: "arc move requires a non-zero I/J offset".to_owned(),
                    });
                }
                if !seen.contains_key(&'I') {
                    frame.words.push(('I', 0.0));
                }
                if !seen.contains_key(&'J') {
                    frame.words.push(('J', 0.0));
                }
                Some(Point2::new(state.pos.x + i, state.pos.y + j))
            } else {
                None
            };
            // An `F` word on the motion line itself feeds *this* move
            // (controllers apply it immediately), so update the modal state
            // before snapshotting it into the segment.
            if let Some(feed) = seen.get(&'F').copied() {
                state.feed = Some(feed);
            }
            let target = resolve_target(state.pos, &seen, state.absolute);
            toolpath.segments.push(Segment {
                id: state.next_id,
                kind: mode,
                from: state.pos,
                to: target,
                center,
                feed: state.feed,
                line,
            });
            state.next_id += 1;
            state.pos = target;
        }
        if let Some(feed) = seen.get(&'F').copied() {
            state.feed = Some(feed);
        }
        toolpath.codes.push(frame);
    }

    // Leading comment-only frames are also exposed as free-form header text
    // (program provenance survives transforms that rewrite the body).
    toolpath.header_comments = toolpath
        .codes
        .iter()
        .take_while(|frame| frame.codes.is_empty() && frame.words.is_empty())
        .filter_map(|frame| frame.comment.clone())
        .collect();
    toolpath.units = units;
    Ok(toolpath)
}

/// Canonicalize a G/M token like `"G04"` to `"G4"` while keeping fractional
/// codes (`"G38.2"`) intact; the numeric `Code::G` path already canonicalizes
/// itself, so this only serves the `Code::Other` fallback.
fn strip_zero_padding(token: &str) -> String {
    let (prefix, digits) = token.split_at(1);
    if digits.contains('.') {
        return token.to_owned();
    }
    match digits.parse::<u16>() {
        Ok(n) => format!("{prefix}{n}"),
        Err(_) => token.to_owned(),
    }
}

/// Split one source line into command text and comment text. Handles `;`
/// to end-of-line and `( … )` blocks; both comment forms are collected and
/// joined with `" | "` in source order.
fn split_comments(line: &str) -> (String, Option<String>) {
    let mut command = String::new();
    let mut comments: Vec<String> = Vec::new();
    let mut chars = line.chars();
    while let Some(ch) = chars.next() {
        if ch == ';' {
            comments.push(chars.by_ref().collect::<String>().trim().to_owned());
            break;
        }
        if ch == '(' {
            let mut block = String::new();
            for inner in chars.by_ref() {
                if inner == ')' {
                    break;
                }
                block.push(inner);
            }
            comments.push(block.trim().to_owned());
            command.push(' ');
            continue;
        }
        command.push(ch);
    }
    let joined = comments
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    let comment = if joined.is_empty() { None } else { Some(joined) };
    (command, comment)
}

/// Compute the absolute target point of a motion line from address words.
fn resolve_target(pos: Point3, words: &HashMap<char, f64>, absolute: bool) -> Point3 {
    let mut target = pos;
    for (letter, index) in AXIS_WORDS {
        if let Some(value) = words.get(&letter) {
            let addressed = if absolute { *value } else { coord_at(pos, index) + value };
            set_coord(&mut target, index, addressed);
        }
    }
    target
}

/// Read one coordinate of a [`Point3`] by the [`AXIS_WORDS`] index.
fn coord_at(p: Point3, index: usize) -> f64 {
    match index {
        0 => p.x,
        1 => p.y,
        2 => p.z,
        3 => p.a.unwrap_or(0.0),
        4 => p.b.unwrap_or(0.0),
        _ => p.c.unwrap_or(0.0),
    }
}

/// Write one coordinate of a [`Point3`] by the [`AXIS_WORDS`] index. Rotary
/// axes store `Some(value)` so the emitter keeps them explicit.
fn set_coord(p: &mut Point3, index: usize, value: f64) {
    match index {
        0 => p.x = value,
        1 => p.y = value,
        2 => p.z = value,
        3 => p.a = Some(value),
        4 => p.b = Some(value),
        _ => p.c = Some(value),
    }
}

/// Render the IR back to G-code text (see module docs for the strategy).
pub fn emit_toolpath(toolpath: &Toolpath) -> RosettaResult<String> {
    // Segment view grouped by source line, order preserved per line.
    let mut segments_by_line: HashMap<usize, Vec<&Segment>> = HashMap::new();
    for segment in &toolpath.segments {
        segments_by_line.entry(segment.line).or_default().push(segment);
    }
    let mut out = String::new();
    // Units header keeps the program self-describing.
    out.push_str(match toolpath.units {
        Units::Millimeters => "G21",
        Units::Inches => "G20",
    });
    out.push('\n');
    for frame in &toolpath.codes {
        let comment_suffix = frame
            .comment
            .as_deref()
            .map(|text| format!(" ; {text}"))
            .unwrap_or_default();
        if let Some(substitute) = &frame.substitute {
            // TRF-M-06: emulated replacement lines in place of the original;
            // the frame itself keeps the original for artifact tracing.
            for line in substitute {
                out.push_str(line.trim_end());
                out.push('\n');
            }
            continue;
        }
        if let Some(segments) = segments_by_line.get(&frame.line) {
            for segment in segments {
                out.push_str(&render_segment(segment, frame)?);
                out.push_str(&comment_suffix);
                out.push('\n');
            }
            continue;
        }
        // Pure command line: canonical codes + address words in source order.
        let mut parts: Vec<String> = frame.codes.iter().map(|c| c.canonical()).collect();
        for (letter, value) in &frame.words {
            parts.push(format!("{letter}{}", fmt_num(*value)));
        }
        if parts.is_empty() {
            // Comment-only frame: re-emit it as a real comment line, never
            // as bare text that a parser would misread as address words.
            if let Some(text) = &frame.comment {
                out.push_str(&format!("; {text}"));
            }
            out.push('\n');
        } else {
            out.push_str(&parts.join(" "));
            out.push_str(&comment_suffix);
            out.push('\n');
        }
    }
    Ok(out)
}

/// Render one motion segment into a self-contained line. Axis coordinates
/// come from the (transformed) geometry; `G90` is prepended to motion lines
/// so emission never depends on modal state (idempotent round-trips).
fn render_segment(segment: &Segment, frame: &CodeFrame) -> RosettaResult<String> {
    let kind_token = match segment.kind {
        SegmentKind::Rapid => "G0",
        SegmentKind::Linear => "G1",
        SegmentKind::ArcCw => "G2",
        SegmentKind::ArcCcw => "G3",
        SegmentKind::Dwell => "G4",
    };
    let mut parts = vec![format!("G90 {kind_token}")];
    if segment.kind == SegmentKind::Dwell {
        let ms = segment.feed.unwrap_or(0.0);
        parts.push(format!("P{}", fmt_num(ms)));
    } else {
        parts.push(format!("X{}", fmt_num(segment.to.x)));
        parts.push(format!("Y{}", fmt_num(segment.to.y)));
        parts.push(format!("Z{}", fmt_num(segment.to.z)));
        for (letter, value) in [('A', segment.to.a), ('B', segment.to.b), ('C', segment.to.c)] {
            if let Some(angle) = value {
                parts.push(format!("{letter}{}", fmt_num(angle)));
            }
        }
        if segment.kind.is_arc() {
            let center = segment.center.ok_or_else(|| RosettaError::Geometry {
                reason: format!("arc segment {} has no center", segment.id),
            })?;
            parts.push(format!("I{}", fmt_num(center.x - segment.from.x)));
            parts.push(format!("J{}", fmt_num(center.y - segment.from.y)));
        }
        if let Some(feed) = segment.feed {
            parts.push(format!("F{}", fmt_num(feed)));
        }
        // Carry over frame words the segment model does not store
        // (spindle speed, program numbers …) so nothing is silently lost.
        // Letters rendered above are skipped; `F` is carried only when the
        // segment itself has no explicit feed (modal source of truth then
        // stays with the word list).
        for (letter, value) in &frame.words {
            let rendered = matches!(letter, 'X' | 'Y' | 'Z' | 'A' | 'B' | 'C' | 'I' | 'J')
                || (*letter == 'F' && segment.feed.is_some());
            if !rendered {
                parts.push(format!("{letter}{}", fmt_num(*value)));
            }
        }
    }
    Ok(parts.join(" "))
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
        let m6 = tp.codes.iter().position(|f| f.mentions(&Code::M(6))).unwrap();
        tp.codes[m6].substitute = Some(vec!["(tool_change begin)".into(), "M6".into()]);
        let text = GcodeIo::new().emit(&tp).unwrap();
        assert!(text.contains("(tool_change begin)\nM6\n"), "text: {text}");
        // The original frame still describes its own line for tracing.
        assert_eq!(tp.codes[m6].line, 8);
    }

    #[test]
    fn inline_comments_and_lowercase_tokens_parse() {
        let tp = GcodeIo::new().parse("g1 x2 (rapid-ish) y3 ; feed down\n").unwrap();
        assert_eq!(tp.segments[0].to, Point3::new(2.0, 3.0, 0.0));
        assert_eq!(tp.codes[0].comment.as_deref(), Some("rapid-ish | feed down"));
    }

    #[test]
    fn fractional_gcodes_survive_as_other() {
        // G38.2 (probe) cannot be Code::G(u16); it must round-trip verbatim.
        let tp = GcodeIo::new().parse("G91 G38.2 Z-10 F100\n").unwrap();
        assert!(tp.codes[0].mentions(&Code::Other("G38.2".into())));
        assert_eq!(tp.segments[0].kind, SegmentKind::Linear);
    }

    #[test]
    fn malformed_word_reports_line() {
        let error = GcodeIo::new().parse("G1 Xfoo\n").unwrap_err();
        assert!(matches!(error, RosettaError::Parse { line: 1, .. }));
    }
}
