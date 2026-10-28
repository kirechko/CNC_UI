//! Rust port of the browser G-code toolpath parser (`web/js/gcode.js`).
//!
//! The backend uses this parser to build the universal [`Toolpath`] that is
//! sent to the UI, so the traversed path is computed once on the server and
//! not inferred in the browser. Behaviour intentionally matches the JS
//! reference:
//! - comments `( ... )` and `; ...` are stripped; blank lines are skipped;
//! - `program_line` is 1-based and counts only non-empty lines;
//! - modal `G90`/`G91` (absolute/relative), `G20`/`G21` (inch/mm),
//!   `G0`–`G3` motion and `F` feed rate are tracked across lines;
//! - arcs (`G2`/`G3` with `I`/`J`) are split into chords, with a count of
//!   `min(384, max(16, ceil(|sweep| * radius / 1.5)))`;
//! - a full circle is produced when the arc end point equals its start.
//!
//! Distances are reported in millimetres.
//!
//! Children:
//! - [`state`] - the modal [`ParserState`](state::ParserState) that turns
//!   comment-stripped G-code lines into [`ToolpathSegment`]s, including arcs.
//! - [`lex`] - word/number lexing, comment stripping and small point helpers.
//! - [`tests`] - unit tests ported verbatim from the original inline module.

use crate::toolpath::Toolpath;

mod lex;
mod state;
#[cfg(test)]
mod tests;

use state::ParserState;

/// Maximum number of chords used for one arc.
const MAX_ARC_CHORDS: usize = 384;
/// Minimum number of chords used for one arc.
const MIN_ARC_CHORDS: usize = 16;
/// Target chord length in millimetres used to choose the chord count.
const ARC_CHORD_LENGTH_MM: f64 = 1.5;

/// Parses G-code source text into a [`Toolpath`] with cumulative distances.
///
/// `source` may use `\n` or `\r\n` line endings. The returned toolpath
/// contains every rapid, linear and arc-chord segment in execution order.
pub fn parse_toolpath(source: &str) -> Toolpath {
    let mut state = ParserState::new();
    for raw_line in source.split('\n') {
        // Strip a trailing '\r' left over from CRLF line endings.
        let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        state.process_line(raw_line);
    }
    state.finish()
}
