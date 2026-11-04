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
function trajectoryColors(laserMode = false) {
  return {
    // Rapid moves are translucent gray traversals, distinct from cutting moves.
    rapid: themeAlpha("--traj-rapid", 0.58),
    linear: themeToken("--traj-linear"),
    arc_clockwise: themeToken("--traj-arc"),
    arc_counter_clockwise: themeToken("--traj-arc"),
    // A distinct active-theme accent makes traversed distance easy to identify.
    completed: laserMode ? "#f2f3f4" : themeToken("--accent-bright"),
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
function drawSegmentPart(context, project, segment, startRatio, endRatio, alpha, colors, completed = false, laserStyle = null) {
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
  context.strokeStyle = completed ? (laserStyle?.completedColor ?? colors.completed) : (laserStyle?.color ?? (colors[segment.kind] || colors.linear));
  context.globalAlpha = completed ? 0.98 : laserStyle?.alpha ?? alpha;
  context.lineWidth = laserStyle ? Math.max(0.35, laserStyle.width) : isRapid ? 1 : 1.7;
  context.setLineDash(isRapid ? [4, 5] : []);
  context.stroke();
}

/**
 * Draws profile-appropriate toolpath strokes and splits at backend progress distance.
 * In laser mode a segment's explicit `laser_power` is required for power-scaled density;
 * missing per-segment power remains thin gray, never inferred from the live setpoint.
 *
 * @param {object} options - Rendering options.
 * @param {CanvasRenderingContext2D} options.context - Target 2D context.
 * @param {Array<object>} options.segments - Backend toolpath segments from `/api/toolpath`
 *   (`from`, `to`, `kind`, `start_distance_mm`, `length_mm`).
 * @param {object|null} options.progress - Backend `ToolpathProgress` (may be null).
 * @param {(point: number[]) => {x: number, y: number}} options.project - Projection function.
 */
export function drawTrajectorySegments({ context, segments, progress, project, laserMode = false, laserPower = null, laserPowerMaximum = null, laserEnabled = false }) {
  const traversed = Number.isFinite(progress?.path_distance_mm)
    ? progress.path_distance_mm
    : null;
  // Resolve theme colors once per draw pass, not per segment.
  const colors = trajectoryColors(laserMode);

  const maxPower = Number(laserPowerMaximum);
  const programPowerValues = segments
    .map((segment) => Number(segment.laser_power))
    .filter((value) => Number.isFinite(value) && value >= 0);
  const hasPowerData = laserMode && programPowerValues.length > 0;
  const minimumProgramPower = hasPowerData ? Math.min(...programPowerValues) : 0;
  const maximumProgramPower = hasPowerData ? Math.max(...programPowerValues) : 0;
  for (const segment of segments) {
    const segmentPower = Number(segment.laser_power);
    const power = hasPowerData && Number.isFinite(segmentPower)
      ? maximumProgramPower > minimumProgramPower
        ? Math.min(1, Math.max(0, (segmentPower - minimumProgramPower) / (maximumProgramPower - minimumProgramPower)))
        : 0.5
      : null;
    const laserStyle = laserMode
      ? {
          color: "#51565c",
          completedColor: "#000000",
          // Map normalized laser power directly to visible stroke thickness; missing power is a thin gray line.
          alpha: power == null ? 0.88 : 0.35 + power * 0.65,
          width: power == null ? 1 : 0.65 + power * 2.1,
        }
      : null;
    if (traversed === null) {
      drawSegmentPart(context, project, segment, 0, 1, 1, colors, false, laserStyle);
      continue;
    }

    const length = segment.length_mm;
    const completedLength = Math.max(
      0,
      Math.min(length, traversed - segment.start_distance_mm),
    );
    const splitRatio = length > 1e-9 ? completedLength / length : 0;
    drawSegmentPart(context, project, segment, 0, splitRatio, 0.22, colors, true, laserStyle);
    drawSegmentPart(context, project, segment, splitRatio, 1, 1, colors, false, laserStyle);
  }

  context.setLineDash([]);
  context.globalAlpha = 1;
}
