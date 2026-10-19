function boundsFromPoints(points) {
  const min = [Infinity, Infinity, Infinity];
  const max = [-Infinity, -Infinity, -Infinity];

  for (const point of points) {
    for (let axis = 0; axis < 3; axis++) {
      min[axis] = Math.min(min[axis], point[axis]);
      max[axis] = Math.max(max[axis], point[axis]);
    }
  }

  return {
    min,
    max,
    center: min.map((value, axis) => (value + max[axis]) / 2),
  };
}

export function calculateSceneBounds(segments, stockMesh) {
  const points = segments.flatMap(({ from, to }) => [from, to]);
  if (stockMesh) {
    for (const triangle of stockMesh.triangles) points.push(...triangle);
  }
  points.push([0, 0, 0]);

  return boundsFromPoints(points);
}

export function calculateTrajectoryBounds(segments, stockMesh) {
  const points = segments.flatMap(({ from, to }) => [from, to]);
  if (points.length === 0) {
    return calculateSceneBounds(segments, stockMesh);
  }

  const bounds = boundsFromPoints(points);

  // Give flat paths a visible stock thickness while preserving their center.
  for (let axis = 0; axis < 3; axis++) {
    if (bounds.max[axis] - bounds.min[axis] < 1) {
      const center = (bounds.min[axis] + bounds.max[axis]) / 2;
      bounds.min[axis] = center - 0.5;
      bounds.max[axis] = center + 0.5;
    }
  }

  bounds.center = bounds.min.map((value, axis) => (value + bounds.max[axis]) / 2);
  return bounds;
}

export function rotatePoint(point, yaw, pitch) {
  const [x, y, z] = point;
  const cosYaw = Math.cos(yaw);
  const sinYaw = Math.sin(yaw);
  const horizontal = cosYaw * x - sinYaw * y;
  const depthPlane = sinYaw * x + cosYaw * y;

  return [
    horizontal,
    Math.cos(pitch) * z - Math.sin(pitch) * depthPlane,
    Math.sin(pitch) * z + Math.cos(pitch) * depthPlane,
  ];
}

export function makeProjector(width, height, bounds, camera) {
  const rotated = [
    ...bounds.min.map((_, index) => {
      const point = [bounds.min[0], bounds.min[1], bounds.min[2]];
      point[index] = bounds.max[index];
      return rotatePoint(
        point.map((value, axis) => value - bounds.center[axis]),
        camera.yaw,
        camera.pitch,
      );
    }),
    ...[
      [bounds.min[0], bounds.min[1], bounds.min[2]],
      [bounds.min[0], bounds.min[1], bounds.max[2]],
      [bounds.min[0], bounds.max[1], bounds.min[2]],
      [bounds.min[0], bounds.max[1], bounds.max[2]],
      [bounds.max[0], bounds.min[1], bounds.min[2]],
      [bounds.max[0], bounds.min[1], bounds.max[2]],
      [bounds.max[0], bounds.max[1], bounds.min[2]],
      [bounds.max[0], bounds.max[1], bounds.max[2]],
    ].map((point) =>
      rotatePoint(
        point.map((value, axis) => value - bounds.center[axis]),
        camera.yaw,
        camera.pitch,
      ),
    ),
  ];

  const extentX = Math.max(...rotated.map((point) => Math.abs(point[0])), 1);
  const extentY = Math.max(...rotated.map((point) => Math.abs(point[1])), 1);
  const scale =
    Math.min((width - 76) / (2 * extentX), (height - 76) / (2 * extentY)) *
    camera.zoom;
  const focalLength = Math.max(width, height) * 2.5;

  return (point) => {
    const rotatedPoint = rotatePoint(
      point.map((value, axis) => value - bounds.center[axis]),
      camera.yaw,
      camera.pitch,
    );
    const depthScale = camera.perspective
      ? focalLength / Math.max(focalLength + rotatedPoint[2], focalLength * 0.35)
      : 1;

    return {
      x: width / 2 + camera.panX + rotatedPoint[0] * scale * depthScale,
      y: height / 2 + camera.panY - rotatedPoint[1] * scale * depthScale,
      depth: rotatedPoint[2],
    };
  };
}

export function makeTrajectoryProjector(project) {
  return project;
}
