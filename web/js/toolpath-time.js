/**
 * Returns the estimated machining time of one backend toolpath segment.
 *
 * Rapid moves are limited only by the axis maximum velocities; feed moves use
 * the segment feed rate, further limited by axis maximum velocities.
 *
 * @param {object} segment - Backend toolpath segment (`from`, `to`, `kind`, `feed_rate`, `length_mm`).
 * @param {object} configuration - Machine configuration with `axes[].max_velocity`.
 * @returns {number} Duration in seconds.
 */
function segmentDuration(segment, configuration) {
  const length = segment.length_mm;
  if (length <= 1e-9) return 0;

  const axes = Array.isArray(configuration?.axes) ? configuration.axes : [];
  const fallbackFeed = Number.isFinite(segment.feed_rate) && segment.feed_rate > 0
    ? segment.feed_rate
    : 1000;
  let speed = segment.kind === "rapid" ? Infinity : fallbackFeed;

  const delta = {
    X: segment.to.x - segment.from.x,
    Y: segment.to.y - segment.from.y,
    Z: segment.to.z - segment.from.z,
  };

  for (const axis of ["X", "Y", "Z"]) {
    const component = Math.abs(delta[axis]) / length;
    if (component <= 1e-9) continue;

    const maxVelocity = Number(
      axes.find((item) => String(item.axis).toUpperCase() === axis)?.max_velocity,
    );
    if (Number.isFinite(maxVelocity) && maxVelocity > 0) {
      speed = Math.min(speed, maxVelocity / component);
    }
  }

  if (!Number.isFinite(speed) || speed <= 0) {
    speed = fallbackFeed;
  }

  return (length / speed) * 60;
}

/**
 * Formats seconds as `HH:MM:SS`.
 *
 * @param {number} seconds - Duration in seconds.
 * @returns {string} Formatted duration.
 */
export function formatDuration(seconds) {
  const total = Math.max(0, Math.ceil(seconds));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const remainder = total % 60;

  return [hours, minutes, remainder]
    .map((value) => String(value).padStart(2, "0"))
    .join(":");
}

/**
 * Formats seconds as `HH:MM:SS.t` with tenths of a second.
 *
 * @param {number} seconds - Duration in seconds.
 * @returns {string} Formatted duration with one fractional digit.
 */
export function formatRemainingDuration(seconds) {
  const tenths = Math.max(0, Math.ceil(seconds * 10));
  const wholeSeconds = Math.floor(tenths / 10);
  const fraction = tenths % 10;
  return `${formatDuration(wholeSeconds)}.${fraction}`;
}

/**
 * Estimates total and remaining machining time from the backend traversed distance.
 *
 * The traversed distance is the backend `progress.path_distance_mm`, so the
 * estimate uses the same source of truth as the drawn trajectory.
 *
 * @param {Array<object>} segments - Backend toolpath segments.
 * @param {object} configuration - Machine configuration with axis limits.
 * @param {number|null} pathDistanceMm - Traversed path distance, or null when unknown.
 * @returns {{totalSeconds: number, remainingSeconds: number, label: string}} Time estimate.
 */
export function estimateToolpathTime(segments, configuration, pathDistanceMm = null) {
  const timedSegments = segments.map((segment) => ({
    seconds: segmentDuration(segment, configuration),
    startDistance: segment.start_distance_mm,
    length: segment.length_mm,
  }));
  const totalSeconds = timedSegments.reduce((sum, item) => sum + item.seconds, 0);

  let remainingSeconds = totalSeconds;
  if (Number.isFinite(pathDistanceMm)) {
    remainingSeconds = timedSegments.reduce((sum, item) => {
      if (item.length <= 1e-9) return sum;
      const done = Math.max(
        0,
        Math.min(1, (pathDistanceMm - item.startDistance) / item.length),
      );
      return sum + item.seconds * (1 - done);
    }, 0);
  }

  return {
    totalSeconds,
    remainingSeconds,
    label: `${formatDuration(remainingSeconds)}/${formatDuration(totalSeconds)}`,
  };
}
