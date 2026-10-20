function segmentLength(segment) {
  return Math.hypot(
    segment.to[0] - segment.from[0],
    segment.to[1] - segment.from[1],
    segment.to[2] - segment.from[2],
  );
}

function segmentDuration(segment, configuration) {
  const length = segmentLength(segment);
  if (length <= 1e-9) return 0;

  const axes = Array.isArray(configuration?.axes) ? configuration.axes : [];
  let speed = segment.type === "rapid"
    ? Infinity
    : Number.isFinite(segment.feedRate) && segment.feedRate > 0
      ? segment.feedRate
      : 1000;

  for (let index = 0; index < 3; index++) {
    const component = Math.abs(segment.to[index] - segment.from[index]) / length;
    if (component <= 1e-9) continue;

    const axis = ["X", "Y", "Z"][index];
    const maxVelocity = Number(
      axes.find((item) => String(item.axis).toUpperCase() === axis)?.max_velocity,
    );
    if (Number.isFinite(maxVelocity) && maxVelocity > 0) {
      speed = Math.min(speed, maxVelocity / component);
    }
  }

  if (!Number.isFinite(speed) || speed <= 0) {
    speed = Number.isFinite(segment.feedRate) && segment.feedRate > 0
      ? segment.feedRate
      : 1000;
  }

  return (length / speed) * 60;
}

function formatDuration(seconds) {
  const total = Math.max(0, Math.ceil(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const remainder = total % 60;

  return [hours, minutes, remainder]
    .map((value) => String(value).padStart(2, "0"))
    .join(":");
}

export function estimateToolpathTime(
  segments,
  configuration,
  currentLine = 0,
  position = null,
) {
  const timedSegments = segments.map((segment) => ({
    segment,
    length: segmentLength(segment),
    seconds: segmentDuration(segment, configuration),
  }));
  const totalSeconds = timedSegments.reduce(
    (sum, item) => sum + item.seconds,
    0,
  );
  const line = Number(currentLine) || 0;
  let remainingSeconds = totalSeconds;

  if (line > 0) {
    const active = timedSegments.filter(
      ({ segment }) => segment.programLine === line,
    );
    const futureSeconds = timedSegments
      .filter(({ segment }) => segment.programLine > line)
      .reduce((sum, item) => sum + item.seconds, 0);

    if (active.length === 0) {
      remainingSeconds = futureSeconds;
    } else {
      const activeLength = active.reduce((sum, item) => sum + item.length, 0);
      let progressDistance = 0;

      if (Array.isArray(position) && position.length >= 3 && activeLength > 1e-9) {
        let precedingLength = 0;
        let closestDistanceSquared = Infinity;

        for (const { segment, length } of active) {
          const delta = segment.to.map((value, axis) => value - segment.from[axis]);
          const lengthSquared = delta.reduce((sum, value) => sum + value * value, 0);
          const projection = lengthSquared > 1e-12
            ? Math.max(0, Math.min(1, position.reduce(
                (sum, value, axis) => sum + (value - segment.from[axis]) * delta[axis],
                0,
              ) / lengthSquared))
            : 0;
          const projected = segment.from.map(
            (value, axis) => value + delta[axis] * projection,
          );
          const distanceSquared = projected.reduce(
            (sum, value, axis) => sum + (value - position[axis]) ** 2,
            0,
          );

          if (distanceSquared < closestDistanceSquared) {
            closestDistanceSquared = distanceSquared;
            progressDistance = precedingLength + length * projection;
          }
          precedingLength += length;
        }
      }

      const activeSeconds = active.reduce((sum, item) => sum + item.seconds, 0);
      const progress = activeLength > 1e-9
        ? Math.max(0, Math.min(1, progressDistance / activeLength))
        : 0;
      remainingSeconds = futureSeconds + activeSeconds * (1 - progress);
    }
  }

  return {
    totalSeconds,
    remainingSeconds,
    label: `${formatDuration(remainingSeconds)}/${formatDuration(totalSeconds)}`,
  };
}
