export function calculateToolpathProgress(
  segments,
  line,
  position,
  previousProgress = null,
  maxAdvance = Infinity,
) {
  if (
    !line ||
    !Array.isArray(position) ||
    position.length < 3 ||
    position.slice(0, 3).some((value) => !Number.isFinite(value))
  ) {
    return previousProgress;
  }

  const cumulativeByLine = new Map();
  const minimumPathDistance = previousProgress?.pathDistance ?? 0;
  const maximumPathDistance = minimumPathDistance + maxAdvance;
  let pathDistance = 0;
  let closest = null;

  for (const segment of segments) {
    const delta = segment.to.slice(0, 3).map(
      (value, axis) => value - segment.from[axis],
    );
    const lengthSquared = delta.reduce((sum, value) => sum + value * value, 0);
    const length = Math.sqrt(lengthSquared);
    const segmentStart = pathDistance;
    const segmentEnd = segmentStart + length;
    const lineDistance = cumulativeByLine.get(segment.programLine) || 0;

    if (
      segment.programLine <= line &&
      segmentEnd >= minimumPathDistance &&
      segmentStart <= maximumPathDistance
    ) {
      let t = 0;
      if (lengthSquared > 1e-12) {
        t = Math.max(0, Math.min(1, position.slice(0, 3).reduce(
          (sum, value, axis) =>
            sum + (value - segment.from[axis]) * delta[axis],
          0,
        ) / lengthSquared));
      }

      const candidatePathDistance = segmentStart + length * t;
      if (
        candidatePathDistance >= minimumPathDistance &&
        candidatePathDistance <= maximumPathDistance
      ) {
        const distanceSquared = position.slice(0, 3).reduce((sum, value, axis) => {
          const projected = segment.from[axis] + delta[axis] * t;
          return sum + (projected - value) ** 2;
        }, 0);

        if (!closest || distanceSquared < closest.distanceSquared) {
          closest = {
            distanceSquared,
            line: segment.programLine,
            distance: lineDistance + length * t,
            pathDistance: candidatePathDistance,
          };
        }
      }
    }

    cumulativeByLine.set(segment.programLine, lineDistance + length);
    pathDistance = segmentEnd;
  }

  return closest
    ? {
        line: closest.line,
        distance: closest.distance,
        pathDistance: closest.pathDistance,
      }
    : previousProgress;
}
