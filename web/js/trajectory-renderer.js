import { toPoint } from "./scene-projection.js";
import { themeAlpha, themeToken } from "./theme-colors.js";

/**
 * Stroke colours keyed by backend `MotionKind` (snake_case JSON values).
 *
 * Resolved lazily from the active theme tokens (web/css/theme-*.css) so the
 * toolpath recolors itself whenever the UI theme changes.
 *
 * @returns {Record<string, string>} Canvas-ready colors per motion kind.
 */
function trajectoryColors() {
  return {
    // Rapid moves are translucent gray traversals, distinct from cutting moves.
    rapid: themeAlpha("--traj-rapid", 0.58),
    linear: themeToken("--traj-linear"),
    arc_clockwise: themeToken("--traj-arc"),
    arc_counter_clockwise: themeToken("--traj-arc"),
  };
}

/**
 * Draws the part of a single segment between two ratios of its length.
 *
 * @param {CanvasRenderingContext2D} context - Target 2D context.
 * @param {(point: number[]) => {x: number, y: number}} project - Projection function.
 * @param {object} segment - Backend toolpath segment (`from`, `to`, `kind`).
 * @param {number} startRatio - Start of the drawn part, 0..1 of the segment length.
 * @param {number} endRatio - End of the drawn part, 0..1 of the segment length.
 * @param {number} alpha - Global alpha used for the stroke.
 */
function drawSegmentPart(context, project, segment, startRatio, endRatio, alpha, colors) {
  if (endRatio - startRatio <= 1e-9) return;

  const from = toPoint(segment.from);
  const to = toPoint(segment.to);
  const interpolate = (ratio) => from.map(
    (value, axis) => value + (to[axis] - value) * ratio,
  );
  const start = project(interpolate(startRatio));
  const end = project(interpolate(endRatio));
  const isRapid = segment.kind === "rapid";

  context.beginPath();
  context.moveTo(start.x, start.y);
  context.lineTo(end.x, end.y);
  context.strokeStyle = colors[segment.kind] || colors.linear;
  context.globalAlpha = alpha;
  context.lineWidth = isRapid ? 1 : 1.7;
  context.setLineDash(isRapid ? [4, 5] : []);
  context.stroke();
}

/**
 * Draws the toolpath, splitting segments at the backend-reported traversed distance.
 *
 * The traversed part (up to `progress.path_distance_mm`) is drawn dimmed and the
 * remaining part at full opacity. When the backend reports no distance, the whole
 * path is drawn at full opacity; the renderer never infers progress itself.
 *
 * @param {object} options - Rendering options.
 * @param {CanvasRenderingContext2D} options.context - Target 2D context.
 * @param {Array<object>} options.segments - Backend toolpath segments from `/api/toolpath`
 *   (`from`, `to`, `kind`, `start_distance_mm`, `length_mm`).
 * @param {object|null} options.progress - Backend `ToolpathProgress` (may be null).
 * @param {(point: number[]) => {x: number, y: number}} options.project - Projection function.
 */
export function drawTrajectorySegments({ context, segments, progress, project }) {
  const traversed = Number.isFinite(progress?.path_distance_mm)
    ? progress.path_distance_mm
    : null;
  // Resolve theme colors once per draw pass, not per segment.
  const colors = trajectoryColors();

  for (const segment of segments) {
    if (traversed === null) {
      drawSegmentPart(context, project, segment, 0, 1, 1, colors);
      continue;
    }

    const length = segment.length_mm;
    const completedLength = Math.max(
      0,
      Math.min(length, traversed - segment.start_distance_mm),
    );
    const splitRatio = length > 1e-9 ? completedLength / length : 0;
    drawSegmentPart(context, project, segment, 0, splitRatio, 0.22, colors);
    drawSegmentPart(context, project, segment, splitRatio, 1, 1, colors);
  }

  context.setLineDash([]);
  context.globalAlpha = 1;
}
