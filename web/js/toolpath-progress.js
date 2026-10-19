export function calculateToolpathProgress(segments, line, position) {
  if (!line) return null;

  let cumulative = 0;
  let closest = null;

  for (const segment of segments) {
    if (segment.programLine !== line) continue;

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
      closest = { distanceSquared, distance: cumulative + length * t };
    }
    cumulative += length;
  }

  return closest ? { line, distance: closest.distance } : null;
}
