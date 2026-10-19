export function rotateStockPoint([x, y, z], angleA, angleB) {
  // A вращает заготовку вокруг X, B — вокруг Y; углы состояния заданы в градусах.
  const cosA = Math.cos(angleA);
  const sinA = Math.sin(angleA);
  const afterA = [x, y * cosA - z * sinA, y * sinA + z * cosA];

  const cosB = Math.cos(angleB);
  const sinB = Math.sin(angleB);
  return [
    afterA[0] * cosB + afterA[2] * sinB,
    afterA[1],
    -afterA[0] * sinB + afterA[2] * cosB,
  ];
}

export function drawStockBox(context, project, bounds, angleA, angleB) {
  const vertices = [
    [bounds.min[0], bounds.min[1], bounds.min[2]],
    [bounds.max[0], bounds.min[1], bounds.min[2]],
    [bounds.max[0], bounds.max[1], bounds.min[2]],
    [bounds.min[0], bounds.max[1], bounds.min[2]],
    [bounds.min[0], bounds.min[1], bounds.max[2]],
    [bounds.max[0], bounds.min[1], bounds.max[2]],
    [bounds.max[0], bounds.max[1], bounds.max[2]],
    [bounds.min[0], bounds.max[1], bounds.max[2]],
  ].map((point) => project(rotateStockPoint(point, angleA, angleB)));
  const faces = [
    [0, 1, 2, 3],
    [4, 5, 6, 7],
    [0, 1, 5, 4],
    [1, 2, 6, 5],
    [2, 3, 7, 6],
    [3, 0, 4, 7],
  ].map((indices) => ({
    points: indices.map((index) => vertices[index]),
    depth: indices.reduce((sum, index) => sum + vertices[index].depth, 0) / 4,
  })).sort((a, b) => a.depth - b.depth);

  for (const face of faces) {
    context.beginPath();
    context.moveTo(face.points[0].x, face.points[0].y);
    face.points.slice(1).forEach((point) => context.lineTo(point.x, point.y));
    context.closePath();
    context.fillStyle = "rgba(66, 164, 212, .12)";
    context.fill();
    context.strokeStyle = "rgba(150, 216, 242, .17)";
    context.lineWidth = 1;
    context.stroke();
  }
}

export function drawStockMesh(context, project, mesh, angleA, angleB) {
  const faces = mesh.triangles.map((triangle) => {
    const points = triangle.map((point) =>
      project(rotateStockPoint(point, angleA, angleB)),
    );
    return {
      points,
      depth: points.reduce((sum, point) => sum + point.depth, 0) / points.length,
    };
  }).sort((a, b) => a.depth - b.depth);

  for (const face of faces) {
    context.beginPath();
    context.moveTo(face.points[0].x, face.points[0].y);
    context.lineTo(face.points[1].x, face.points[1].y);
    context.lineTo(face.points[2].x, face.points[2].y);
    context.closePath();
    context.fillStyle = "rgba(66, 164, 212, .18)";
    context.fill();
    context.strokeStyle = "rgba(150, 216, 242, .34)";
    context.lineWidth = 0.7;
    context.stroke();
  }
}
