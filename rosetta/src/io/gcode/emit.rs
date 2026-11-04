use std::collections::HashMap;

use crate::error::{RosettaError, RosettaResult};
use crate::ir::toolpath::{CodeFrame, Segment, SegmentKind, Toolpath, Units};
use crate::util::fmt_num;

/// Render the IR back to G-code text (see module docs for the strategy).
pub fn emit_toolpath(toolpath: &Toolpath) -> RosettaResult<String> {
    // Segment view grouped by source line, order preserved per line.
    let mut segments_by_line: HashMap<usize, Vec<&Segment>> = HashMap::new();
    for segment in &toolpath.segments {
        segments_by_line
            .entry(segment.line)
            .or_default()
            .push(segment);
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
        for (letter, value) in [
            ('A', segment.to.a),
            ('B', segment.to.b),
            ('C', segment.to.c),
        ] {
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
