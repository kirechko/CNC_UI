function rotateStockPoint([x, y, z], angleA, angleB) {
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
