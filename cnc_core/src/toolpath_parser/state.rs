//! The modal parser state: consumes comment-stripped G-code lines and emits
//! [`ToolpathSegment`]s, tracking units, motion mode, feed rate and position
//! across lines exactly like the browser reference parser.

use super::lex::{distance_3d, read_words, strip_comments, to_position};
use super::{ARC_CHORD_LENGTH_MM, MAX_ARC_CHORDS, MIN_ARC_CHORDS};
use crate::toolpath::{Toolpath, ToolpathSegment};
use crate::MotionKind;

/// Mutable modal state carried from one G-code line to the next.
pub(super) struct ParserState {
    /// Current tool position in millimetres.
    pub(super) position: [f64; 3],
    /// Current rotary positions A and B in degrees (not part of the 2D path).
    pub(super) absolute: bool,
    /// Multiplier from program units to millimetres (1.0 for G21, 25.4 for G20).
    pub(super) units: f64,
    /// Current motion mode: 0 rapid, 1 linear, 2 CW arc, 3 CCW arc.
    pub(super) motion: u8,
    /// Current feed rate in millimetres per minute.
    pub(super) feed_rate: f64,
    /// Count of executable (non-empty) lines seen so far.
    pub(super) program_line: u32,
    /// Segments produced so far.
    pub(super) segments: Vec<ToolpathSegment>,
    /// Cumulative path length from the program start, in millimetres.
    pub(super) total_length: f64,
}

impl ParserState {
    /// Creates a parser with the same defaults as the browser parser.
    pub(super) fn new() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            absolute: true,
            units: 1.0,
            motion: 0,
            feed_rate: 1000.0,
            program_line: 0,
            segments: Vec::new(),
            total_length: 0.0,
        }
    }

    /// Processes one physical line: strips comments, reads words and emits segments.
    pub(super) fn process_line(&mut self, original: &str) {
        let line = strip_comments(original);
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.program_line += 1;

        let words = read_words(line);
        // Last value wins for each letter, matching `new Map(words)` in JS.
        let value_of = |letter: char| -> Option<f64> {
            words
                .iter()
                .rev()
                .find(|(l, _)| *l == letter)
                .map(|(_, v)| *v)
        };

        // Apply modal G codes first; the motion of this line may change them.
        let mut next_motion = self.motion;
        for &(letter, value) in &words {
            if letter != 'G' {
                continue;
            }
            if value == 90.0 {
                self.absolute = true;
            } else if value == 91.0 {
                self.absolute = false;
            } else if value == 20.0 {
                self.units = 25.4;
            } else if value == 21.0 {
                self.units = 1.0;
            } else if value == 0.0 || value == 1.0 || value == 2.0 || value == 3.0 {
                next_motion = value as u8;
            }
        }

        // Feed rate uses the units active on this line, as in the JS parser.
        for &(letter, value) in &words {
            if letter == 'F' && value.is_finite() && value > 0.0 {
                self.feed_rate = value * self.units;
            }
        }

        // Resolve the target point for each linear axis present on the line.
        let mut target = self.position;
        let mut has_axis = false;
        for (index, letter) in ['X', 'Y', 'Z'].iter().enumerate() {
            if let Some(value) = value_of(*letter) {
                has_axis = true;
                let scaled = value * self.units;
                target[index] = if self.absolute {
                    scaled
                } else {
                    self.position[index] + scaled
                };
            }
        }

        let has_arc_center = value_of('I').is_some() || value_of('J').is_some();
        if has_axis && (next_motion == 2 || next_motion == 3) && has_arc_center {
            let i = value_of('I').unwrap_or(0.0) * self.units;
            let j = value_of('J').unwrap_or(0.0) * self.units;
            self.emit_arc(target, next_motion, i, j);
        } else if has_axis {
            let kind = if next_motion == 0 {
                MotionKind::Rapid
            } else {
                MotionKind::Linear
            };
            self.push_segment(self.position, target, kind);
        }

        self.position = target;
        self.motion = next_motion;
    }

    /// Emits the chord segments for an arc from the current position to `target`.
    ///
    /// `i` and `j` are the centre offsets from the current position, already
    /// scaled to millimetres.
    fn emit_arc(&mut self, target: [f64; 3], motion: u8, i: f64, j: f64) {
        let start = self.position;
        let center_x = start[0] + i;
        let center_y = start[1] + j;
        let radius = (start[0] - center_x).hypot(start[1] - center_y);

        // A zero radius degenerates to a straight line, as in the JS parser.
        if radius <= 1e-9 {
            self.push_segment(start, target, MotionKind::Linear);
            return;
        }

        let start_angle = (start[1] - center_y).atan2(start[0] - center_x);
        let end_angle = (target[1] - center_y).atan2(target[0] - center_x);
        let mut sweep = end_angle - start_angle;
        if motion == 2 {
            while sweep >= 0.0 {
                sweep -= std::f64::consts::TAU;
            }
        } else {
            while sweep <= 0.0 {
                sweep += std::f64::consts::TAU;
            }
        }

        // End point equal to start in XY means a full circle.
        if (target[0] - start[0]).hypot(target[1] - start[1]) < 1e-8 {
            sweep = if motion == 2 {
                -std::f64::consts::TAU
            } else {
                std::f64::consts::TAU
            };
        }

        let count_f = (sweep.abs() * radius / ARC_CHORD_LENGTH_MM).ceil();
        let count = (count_f as usize).clamp(MIN_ARC_CHORDS, MAX_ARC_CHORDS);
        let kind = if motion == 2 {
            MotionKind::ArcClockwise
        } else {
            MotionKind::ArcCounterClockwise
        };

        let mut previous = start;
        for step in 1..=count {
            let t = step as f64 / count as f64;
            let angle = start_angle + sweep * t;
            let point = [
                center_x + radius * angle.cos(),
                center_y + radius * angle.sin(),
                start[2] + (target[2] - start[2]) * t,
            ];
            self.push_segment(previous, point, kind);
            previous = point;
        }
    }

    /// Appends one segment, updating cumulative distance and the current position.
    fn push_segment(&mut self, from: [f64; 3], to: [f64; 3], kind: MotionKind) {
        let length = distance_3d(from, to);
        let segment = ToolpathSegment {
            from: to_position(from),
            to: to_position(to),
            kind,
            program_line: self.program_line,
            feed_rate: self.feed_rate,
            start_distance_mm: self.total_length,
            length_mm: length,
        };
        self.total_length += length;
        self.segments.push(segment);
    }

    /// Consumes the parser and returns the completed toolpath.
    pub(super) fn finish(self) -> Toolpath {
        Toolpath {
            segments: self.segments,
            total_length_mm: self.total_length,
        }
    }
}
