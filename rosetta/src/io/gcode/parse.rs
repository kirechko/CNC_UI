use std::collections::HashMap;

use crate::error::{RosettaError, RosettaResult};
use crate::ir::toolpath::{
    Code, CodeFrame, Point2, Point3, Segment, SegmentId, SegmentKind, Toolpath, Units,
};

/// Axis words that define a motion target, in canonical emission order. The
/// `usize` is the index into [`PointSlot`] helpers below.
const AXIS_WORDS: [(char, usize); 6] = [('X', 0), ('Y', 1), ('Z', 2), ('A', 3), ('B', 4), ('C', 5)];

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
                    let code = Code::parse(&upper)
                        .unwrap_or_else(|| Code::Other(strip_zero_padding(&upper)));
                    if let Code::Other(other) = &code {
                        // Decimal G-codes (for example G38.2) are valid and
                        // preserved for capability analysis. Integer codes too
                        // large for the IR's u16 domain are invalid source.
                        let suffix = other.get(1..).unwrap_or_default();
                        let is_integer_code =
                            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit());
                        if is_integer_code {
                            return Err(RosettaError::Parse {
                                line,
                                reason: format!(
                                    "G/M code number is outside the supported range: {other}"
                                ),
                            });
                        }
                    }
                    codes.push(code);
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
        let has_axes = AXIS_WORDS
            .iter()
            .any(|(letter, _)| seen.contains_key(letter));
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
    let comment = if joined.is_empty() {
        None
    } else {
        Some(joined)
    };
    (command, comment)
}

/// Compute the absolute target point of a motion line from address words.
fn resolve_target(pos: Point3, words: &HashMap<char, f64>, absolute: bool) -> Point3 {
    let mut target = pos;
    for (letter, index) in AXIS_WORDS {
        if let Some(value) = words.get(&letter) {
            let addressed = if absolute {
                *value
            } else {
                coord_at(pos, index) + value
            };
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
