//! Nearest-segment geometry helpers backing [`Toolpath::locate`],
//! [`Toolpath::locate_on_line`] and [`Toolpath::locate_forward`].
//!
//! Everything here is crate-internal: the public surface stays on the
//! [`Toolpath`](super::Toolpath) methods so callers never see raw matches.

use super::{ToolpathProgress, ToolpathSegment, PATH_TOLERANCE_MM};

/// Nearest-segment match: squared distance, segment reference and clamped fraction.
pub(super) type SegmentMatch<'a> = (f64, &'a ToolpathSegment, f64);

/// Returns the segment closest to `position` among `segments`, or `None` when empty.
pub(super) fn nearest_segment<'a>(
    segments: impl Iterator<Item = &'a ToolpathSegment>,
    position: &crate::Position,
) -> Option<SegmentMatch<'a>> {
    let mut best: Option<SegmentMatch<'a>> = None;
    for segment in segments {
        let (distance_sq, fraction) = closest_fraction_on_segment(segment, position);
        if best.map_or(true, |(current, _, _)| distance_sq < current) {
            best = Some((distance_sq, segment, fraction));
        }
    }
    best
}

/// Converts a nearest-segment match into progress, applying the path tolerance.
pub(super) fn progress_from_match(
    matched: SegmentMatch<'_>,
    position: &crate::Position,
) -> ToolpathProgress {
    let (distance_sq, segment, fraction) = matched;
    if distance_sq.sqrt() > PATH_TOLERANCE_MM {
        return ToolpathProgress {
            program_line: None,
            path_distance_mm: None,
            tool_position: Some(position.clone()),
        };
    }
    ToolpathProgress {
        program_line: Some(segment.program_line),
        path_distance_mm: Some(segment.start_distance_mm + segment.length_mm * fraction),
        tool_position: Some(position.clone()),
    }
}

/// Returns the squared distance from `position` to the segment and the
/// clamped fraction (0..=1) of the closest point along the segment.
pub(super) fn closest_fraction_on_segment(
    segment: &ToolpathSegment,
    position: &crate::Position,
) -> (f64, f64) {
    let (ax, ay, az) = (segment.from.x, segment.from.y, segment.from.z);
    let (dx, dy, dz) = (segment.to.x - ax, segment.to.y - ay, segment.to.z - az);
    let length_sq = dx * dx + dy * dy + dz * dz;
    let fraction = if length_sq <= 1e-12 {
        0.0
    } else {
        let px = position.x - ax;
        let py = position.y - ay;
        let pz = position.z - az;
        ((px * dx + py * dy + pz * dz) / length_sq).clamp(0.0, 1.0)
    };
    let cx = ax + dx * fraction - position.x;
    let cy = ay + dy * fraction - position.y;
    let cz = az + dz * fraction - position.z;
    (cx * cx + cy * cy + cz * cz, fraction)
}
