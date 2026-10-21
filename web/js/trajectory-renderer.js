const TRAJECTORY_COLORS = {
  rapid: "rgba(153, 168, 174, .58)",
  linear: "#55bd91",
  "arc-cw": "#f1aa58",
  "arc-ccw": "#f1aa58",
};

function drawSegmentPart(context, project, segment, startRatio, endRatio, alpha) {
  if (endRatio - startRatio <= 1e-9) return;

  const interpolate = (ratio) => segment.from.map(
    (value, axis) => value + (segment.to[axis] - value) * ratio,
  );
  const from = project(interpolate(startRatio));
  const to = project(interpolate(endRatio));

  context.beginPath();
  context.moveTo(from.x, from.y);
  context.lineTo(to.x, to.y);
  context.strokeStyle = TRAJECTORY_COLORS[segment.type] || TRAJECTORY_COLORS.linear;
  context.globalAlpha = alpha;
  context.lineWidth = segment.type === "rapid" ? 1 : 1.7;
  context.setLineDash(segment.type === "rapid" ? [4, 5] : []);
  context.stroke();
}

export function drawTrajectorySegments({
  context,
  segments,
  currentProgramLine,
  activeProgress,
  project,
}) {
  let pathDistance = 0;
  let activeLineDistance = 0;

  for (const segment of segments) {
    const delta = segment.to.map((value, axis) => value - segment.from[axis]);
    const length = Math.sqrt(delta.reduce((sum, value) => sum + value * value, 0));

    if (Number.isFinite(activeProgress?.pathDistance)) {
      const completedLength = Math.max(
        0,
        Math.min(length, activeProgress.pathDistance - pathDistance),
      );
      const splitRatio = length > 1e-9 ? completedLength / length : 0;
      drawSegmentPart(context, project, segment, 0, splitRatio, 0.22);
      drawSegmentPart(context, project, segment, splitRatio, 1, 1);
    } else if (segment.programLine < currentProgramLine) {
      drawSegmentPart(context, project, segment, 0, 1, 0.22);
    } else if (activeProgress && segment.programLine === activeProgress.line) {
      const completedLength = Math.max(
        0,
        Math.min(length, activeProgress.distance - activeLineDistance),
      );
      const splitRatio = length > 1e-9 ? completedLength / length : 0;
      drawSegmentPart(context, project, segment, 0, splitRatio, 0.22);
      drawSegmentPart(context, project, segment, splitRatio, 1, 1);
      activeLineDistance += length;
    } else {
      drawSegmentPart(context, project, segment, 0, 1, 1);
    }

    pathDistance += length;
  }

  context.setLineDash([]);
  context.globalAlpha = 1;
}
