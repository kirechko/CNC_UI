import { themeAlpha } from "./theme-colors.js";

/**
 * Rotates a stock vertex by the A (around X) and B (around Y) table angles.
 * State angles are in degrees already converted to radians by the caller.
 */
export function rotateStockPoint([x, y, z], angleA, angleB) {
  // A rotates the stock around X and B around Y; state angles are in degrees.
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

  // Box faces use theme tokens; the low alpha keeps the stock ghost-like.
  const fill = themeAlpha("--stock-fill", 0.12);
  const edge = themeAlpha("--stock-edge", 0.17);
  for (const face of faces) {
    context.beginPath();
    context.moveTo(face.points[0].x, face.points[0].y);
    face.points.slice(1).forEach((point) => context.lineTo(point.x, point.y));
    context.closePath();
    context.fillStyle = fill;
    context.fill();
    context.strokeStyle = edge;
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

  // Mesh triangles are slightly denser than the bounding box faces.
  const fill = themeAlpha("--stock-fill", 0.18);
  const edge = themeAlpha("--stock-edge", 0.34);
  for (const face of faces) {
    context.beginPath();
    context.moveTo(face.points[0].x, face.points[0].y);
    context.lineTo(face.points[1].x, face.points[1].y);
    context.lineTo(face.points[2].x, face.points[2].y);
    context.closePath();
    context.fillStyle = fill;
    context.fill();
    context.strokeStyle = edge;
    context.lineWidth = 0.7;
    context.stroke();
  }
}
