export function calculateToolpathProgress(segments, line, position) {
  if (
    !line ||
    !Array.isArray(position) ||
    position.length < 3 ||
    position.slice(0, 3).some((value) => !Number.isFinite(value))
  ) {
    return null;
  }

  const cumulativeByLine = new Map();
  let closest = null;

  for (const segment of segments) {
    if (segment.programLine > line) continue;

    const segmentLine = segment.programLine;
    const cumulative = cumulativeByLine.get(segmentLine) || 0;
    const delta = segment.to.map((value, axis) => value - segment.from[axis]);
    const lengthSquared = delta.reduce((sum, value) => sum + value * value, 0);
    const length = Math.sqrt(lengthSquared);
    let t = 0;

    if (lengthSquared > 1e-12) {
      const relative = position.map((value, axis) => value - segment.from[axis]);
      t = Math.max(0, Math.min(1,
        relative.reduce((sum, value, axis) => sum + value * delta[axis], 0) / lengthSquared,
      ));
    }

    const projected = segment.from.map((value, axis) => value + delta[axis] * t);
    const distanceSquared = projected.reduce(
      (sum, value, axis) => sum + (value - position[axis]) ** 2,
      0,
    );

    if (!closest || distanceSquared < closest.distanceSquared) {
      closest = {
        distanceSquared,
        line: segmentLine,
        distance: cumulative + length * t,
      };
    }
    cumulativeByLine.set(segmentLine, cumulative + length);
  }

  return closest ? { line: closest.line, distance: closest.distance } : null;
}
