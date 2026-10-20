export function parseToolpath(source) {
  const segments = [];
  let position = [0, 0, 0];
  let absolute = true;
  let units = 1;
  let motion = 0;
  let feedRate = 1000;
  let programLine = 0;

  for (const originalLine of source.split(/\r?\n/)) {
    const line = originalLine.replace(/\([^)]*\)/g, "").split(";")[0].trim();
    if (!line) continue;
    programLine += 1;
    const words = [...line.matchAll(/([A-Za-z])\s*([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)/g)]
      .map((match) => [match[1].toUpperCase(), Number(match[2])]);
    const values = new Map(words);
    let nextMotion = motion;

    for (const [letter, value] of words) {
      if (letter !== "G") continue;
      if (value === 90) absolute = true;
      else if (value === 91) absolute = false;
      else if (value === 20) units = 25.4;
      else if (value === 21) units = 1;
      else if ([0, 1, 2, 3].includes(value)) nextMotion = value;
    }

    for (const [letter, value] of words) {
      if (letter === "F" && Number.isFinite(value) && value > 0) {
        feedRate = value * units;
      }
    }

    const target = [...position];
    let hasAxis = false;
    for (const [index, letter] of ["X", "Y", "Z", "A", "B"].entries()) {
      if (!values.has(letter)) continue;
      hasAxis = true;
      const value = values.get(letter) * units;
      target[index] = absolute ? value : position[index] + value;
    }

    if (hasAxis && (nextMotion === 2 || nextMotion === 3) &&
        (values.has("I") || values.has("J"))) {
      const centerX = position[0] + (values.get("I") || 0) * units;
      const centerY = position[1] + (values.get("J") || 0) * units;
      const radius = Math.hypot(position[0] - centerX, position[1] - centerY);

      if (radius > 1e-9) {
        const startAngle = Math.atan2(position[1] - centerY, position[0] - centerX);
        const endAngle = Math.atan2(target[1] - centerY, target[0] - centerX);
        let sweep = endAngle - startAngle;
        if (nextMotion === 2) {
          while (sweep >= 0) sweep -= Math.PI * 2;
        } else {
          while (sweep <= 0) sweep += Math.PI * 2;
        }

        if (Math.hypot(target[0] - position[0], target[1] - position[1]) < 1e-8) {
          sweep = nextMotion === 2 ? -Math.PI * 2 : Math.PI * 2;
        }

        const count = Math.min(384, Math.max(16, Math.ceil(Math.abs(sweep) * radius / 1.5)));
        let previous = position;
        for (let step = 1; step <= count; step++) {
          const t = step / count;
          const angle = startAngle + sweep * t;
          const point = [
            centerX + radius * Math.cos(angle),
            centerY + radius * Math.sin(angle),
            position[2] + (target[2] - position[2]) * t,
          ];
          segments.push({
            from: previous,
            to: point,
            type: nextMotion === 2 ? "arc-cw" : "arc-ccw",
            programLine,
            feedRate,
          });
          previous = point;
        }
      } else {
        segments.push({
          from: position,
          to: target,
          type: "linear",
          programLine,
          feedRate,
        });
      }
    } else if (hasAxis) {
      segments.push({
        from: position,
        to: target,
        type: nextMotion === 0 ? "rapid" : "linear",
        programLine,
        feedRate,
      });
    }

    position = target;
    motion = nextMotion;
  }

  return segments;
}

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

export function estimateToolpathTime(segments, configuration, currentLine = 0, position = null) {
  const durations = segments.map((segment) => segmentDuration(segment, configuration));
  const totalSeconds = durations.reduce((sum, duration) => sum + duration, 0);
  const line = Number(currentLine) || 0;
  let remainingSeconds = totalSeconds;

  if (line > 0) {
    const active = segments
      .map((segment, index) => ({ segment, duration: durations[index] }))
      .filter(({ segment }) => segment.programLine === line);
    const futureSeconds = segments.reduce(
      (sum, segment, index) =>
        sum + (segment.programLine > line ? durations[index] : 0),
      0,
    );

    if (active.length === 0) {
      remainingSeconds = futureSeconds;
    } else {
      let activeLength = 0;
      let progressDistance = 0;

      if (Array.isArray(position) && position.length >= 3) {
        let precedingLength = 0;
        let closestDistanceSquared = Infinity;

        for (const { segment } of active) {
          const delta = segment.to.map((value, axis) => value - segment.from[axis]);
          const lengthSquared = delta.reduce((sum, value) => sum + value * value, 0);
          const length = Math.sqrt(lengthSquared);
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

          activeLength += length;
          if (distanceSquared < closestDistanceSquared) {
            closestDistanceSquared = distanceSquared;
            progressDistance = precedingLength + length * projection;
          }
          precedingLength += length;
        }
      }

      const activeSeconds = active.reduce((sum, item) => sum + item.duration, 0);
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

function segmentLength(segment) {
  return Math.hypot(
    segment.to[0] - segment.from[0],
    segment.to[1] - segment.from[1],
    segment.to[2] - segment.from[2],
  );
}

function segmentSpeed(segment, configuration) {
  const length = segmentLength(segment);
  if (length <= 1e-9) return 0;

  const limits = Array.isArray(configuration?.axes) ? configuration.axes : [];
  let speed = Number.isFinite(segment.feedRate) && segment.feedRate > 0
    ? segment.feedRate
    : 1000;

  for (let index = 0; index < 3; index++) {
    const axis = ["X", "Y", "Z"][index];
    const component = Math.abs(segment.to[index] - segment.from[index]) / length;
    if (component <= 1e-9) continue;

    const axisConfiguration = limits.find(
      (item) => String(item.axis).toUpperCase() === axis,
    );
    const maxVelocity = Number(axisConfiguration?.max_velocity);
    if (Number.isFinite(maxVelocity) && maxVelocity > 0) {
      speed = Math.min(speed, maxVelocity / component);
    }
  }

  return speed;
}

function formatDuration(seconds) {
  const totalSeconds = Math.max(0, Math.ceil(seconds));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const remainder = totalSeconds % 60;
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
  const timedSegments = segments.map((segment) => {
    const length = segmentLength(segment);
    const speed = segmentSpeed(segment, configuration);
    return {
      ...segment,
      length,
      seconds: speed > 0 ? (length / speed) * 60 : 0,
    };
  });
  const totalSeconds = timedSegments.reduce(
    (sum, segment) => sum + segment.seconds,
    0,
  );

  let remainingSeconds = totalSeconds;
  const line = Number(currentLine) || 0;

  if (line > 0 && timedSegments.some((segment) => segment.programLine >= line)) {
    const currentSegments = timedSegments.filter(
      (segment) => segment.programLine === line,
    );
    const futureSeconds = timedSegments
      .filter((segment) => segment.programLine > line)
      .reduce((sum, segment) => sum + segment.seconds, 0);
    const completedSeconds = timedSegments
      .filter((segment) => segment.programLine < line)
      .reduce((sum, segment) => sum + segment.seconds, 0);

    let currentLineProgress = 0;
    if (currentSegments.length && Array.isArray(position) && position.length >= 3) {
      let distanceBeforeClosest = 0;
      let closestDistanceSquared = Infinity;
      let closestProgress = 0;

      for (const segment of currentSegments) {
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
          closestProgress = distanceBeforeClosest +
            segment.length * projection;
        }
        distanceBeforeClosest += segment.length;
      }

      const lineLength = currentSegments.reduce(
        (sum, segment) => sum + segment.length,
        0,
      );
      currentLineProgress = lineLength > 1e-9
        ? Math.min(1, closestProgress / lineLength)
        : 0;
    }

    const currentLineSeconds = currentSegments.reduce(
      (sum, segment) => sum + segment.seconds,
      0,
    );
    remainingSeconds = futureSeconds +
      currentLineSeconds * (1 - currentLineProgress);

    // Для неактивной загруженной программы сохраняем полный остаток.
    if (line <= 1 && completedSeconds === 0 && currentLineProgress === 0) {
      remainingSeconds = totalSeconds;
    }
  } else if (line > 0 && timedSegments.length) {
    remainingSeconds = 0;
  }

  return {
    totalSeconds,
    remainingSeconds,
    label: `${formatDuration(remainingSeconds)}/${formatDuration(totalSeconds)}`,
  };
}
