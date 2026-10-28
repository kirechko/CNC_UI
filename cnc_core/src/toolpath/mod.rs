//! Universal toolpath and progress contract shared by all CNC controllers.
//!
//! The backend parses a G-code program into [`Toolpath`] segments and reports
//! [`ToolpathProgress`] with every machine status. The web UI draws the
//! traversed part of the path up to `path_distance_mm` and never infers
//! progress itself. Controller-specific behaviour (acknowledgement buffering,
//! status latency, GRBL/LinuxCNC/FluidNC differences) is mapped to these
//! fields inside the adapters.
//!
//! Conventions:
//! - All distances and coordinates are in millimetres in the machine
//!   coordinate frame of the active work coordinate system (as produced by
//!   the program's G20/G21 units, converted to millimetres).
//! - `program_line` is 1-based and counts only executable lines, i.e. lines
//!   that remain after comments and blank lines are removed.
//! - Progress fields are `Option`: `None` means "not available for this
//!   controller or state" and must never be guessed by the UI.
//!
//! Children:
//! - [`geometry`] - nearest-segment search helpers backing the `locate*` methods.
//! - [`tests`] - unit tests for locating, serialization shape and round-tripping.

use serde::{Deserialize, Serialize};

use crate::{MotionKind, Position};
use geometry::{closest_fraction_on_segment, nearest_segment, progress_from_match};

mod geometry;
#[cfg(test)]
mod tests;

/// One straight or arc-chord piece of the programmed toolpath.
///
/// Arcs are stored as a sequence of chords, so every segment is a straight
/// line in space and `from`/`to` are its endpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolpathSegment {
    /// Start point of the segment in millimetres.
    pub from: Position,
    /// End point of the segment in millimetres.
    pub to: Position,
    /// Kind of motion that produced this segment (rapid, linear, arc CW/CCW).
    pub kind: MotionKind,
    /// 1-based executable program line that produced this segment.
    pub program_line: u32,
    /// Feed rate active for the segment, in millimetres per minute.
    pub feed_rate: f64,
    /// Cumulative path length from the program start to `from`, in millimetres.
    pub start_distance_mm: f64,
    /// Length of this segment, in millimetres.
    pub length_mm: f64,
}

/// Parsed toolpath of the loaded program with cumulative distances.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Toolpath {
    /// Segments in execution order.
    pub segments: Vec<ToolpathSegment>,
    /// Total path length of all segments, in millimetres.
    pub total_length_mm: f64,
}

/// Maximum distance in millimetres between the tool and the toolpath for the
/// tool to be considered "on the path". Covers arc chord sagitta (about 0.03 mm
/// on the largest arcs) and small controller position jitter. Positions farther
/// away (for example during JOG) report no traversed distance.
pub const PATH_TOLERANCE_MM: f64 = 0.1;

impl Toolpath {
    /// Locates a tool position on the toolpath and returns the matching progress.
    ///
    /// The nearest segment (by 3D distance from `position` to the segment's
    /// line) is selected, and the traversed distance is its start distance
    /// plus the projected offset along that segment. Returns `None` when the
    /// toolpath is empty.
    ///
    /// When the nearest distance exceeds [`PATH_TOLERANCE_MM`], the tool is not
    /// on the path: `program_line` and `path_distance_mm` are `None` so the UI
    /// does not darken the traversed part, while `tool_position` is still reported.
    ///
    /// Limitation: when the path crosses itself, the nearest segment may belong
    /// to a different pass than the one being executed. Adapters that know the
    /// executing program line should prefer segments of that line.
    pub fn locate(&self, position: &crate::Position) -> Option<ToolpathProgress> {
        let best = nearest_segment(self.segments.iter(), position)?;
        Some(progress_from_match(best, position))
    }

    /// Locates a tool position while preferring the segments of the program line
    /// that the controller is currently executing.
    ///
    /// Searching only the executing line removes ambiguity where the path crosses
    /// itself, for example when the tool is lifted straight up from the end of an
    /// arc that shares its XY coordinates with earlier passes. If the executing
    /// line has no segment within [`PATH_TOLERANCE_MM`] (or `program_line` is 0 or
    /// unknown), the search falls back to the whole toolpath as in [`Toolpath::locate`].
    pub fn locate_on_line(
        &self,
        position: &crate::Position,
        program_line: u32,
    ) -> Option<ToolpathProgress> {
        if program_line > 0 {
            let on_line = self
                .segments
                .iter()
                .filter(|segment| segment.program_line == program_line);
            if let Some(best) = nearest_segment(on_line, position) {
                if best.0.sqrt() <= PATH_TOLERANCE_MM {
                    return Some(progress_from_match(best, position));
                }
            }
        }
        self.locate(position)
    }

    /// Locates a tool position on the part of the toolpath that a controller
    /// can still reach, moving forward only.
    ///
    /// Only segments whose program line is at most `max_program_line` are
    /// considered, and only matches whose traversed distance is not behind
    /// `min_distance_mm` (minus [`PATH_TOLERANCE_MM`] of slack). Among the
    /// remaining matches within tolerance, the one with the smallest traversed
    /// distance is returned, so the result never moves back to an earlier pass.
    ///
    /// Returns `None` when no segment in that window lies within tolerance.
    /// Unlike [`Toolpath::locate_on_line`] there is no fallback to the whole path.
    pub fn locate_forward(
        &self,
        position: &crate::Position,
        min_distance_mm: f64,
        max_program_line: u32,
    ) -> Option<ToolpathProgress> {
        let mut best: Option<ToolpathProgress> = None;
        for segment in self
            .segments
            .iter()
            .filter(|segment| segment.program_line <= max_program_line)
        {
            let (distance_sq, fraction) = closest_fraction_on_segment(segment, position);
            if distance_sq.sqrt() > PATH_TOLERANCE_MM {
                continue;
            }
            let distance = segment.start_distance_mm + segment.length_mm * fraction;
            if distance + PATH_TOLERANCE_MM < min_distance_mm {
                continue;
            }
            let is_earlier = best
                .and_then(|current| current.path_distance_mm)
                .map_or(true, |current| distance < current);
            if is_earlier {
                best = Some(ToolpathProgress {
                    program_line: Some(segment.program_line),
                    path_distance_mm: Some(distance),
                    tool_position: Some(position.clone()),
                });
            }
        }
        best
    }
}

/// Universal progress reported with every machine status.
///
/// The UI draws the traversed part of the toolpath up to `path_distance_mm`
/// and places the tool marker at `tool_position`. Both are authoritative
/// backend values; the UI must not compute them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ToolpathProgress {
    /// 1-based executable program line the machine is currently executing.
    /// `None` when no program is running or the controller cannot tell.
    #[serde(default)]
    pub program_line: Option<u32>,
    /// Path length traversed from the program start, in millimetres.
    /// `None` when the controller does not expose a trustworthy position.
    #[serde(default)]
    pub path_distance_mm: Option<f64>,
    /// Current tool position in the machine coordinate frame of the toolpath.
    /// `None` when the controller does not expose a trustworthy position.
    #[serde(default)]
    pub tool_position: Option<Position>,
}

impl ToolpathProgress {
    /// Creates progress that carries no information, used when no program
    /// is loaded or the adapter cannot report progress.
    pub fn unavailable() -> Self {
        Self::default()
    }

    /// Returns `true` when at least one progress field is known.
    pub fn is_available(&self) -> bool {
        self.program_line.is_some()
            || self.path_distance_mm.is_some()
            || self.tool_position.is_some()
    }
}
