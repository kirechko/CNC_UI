import { rotatePoint } from "./scene-projection.js";
import { resizeCanvas } from "./canvas-utils.js";
import { themeAlpha, themeToken } from "./theme-colors.js";

const VERTICES = [
  [-1, -1, -1], [1, -1, -1], [1, 1, -1], [-1, 1, -1],
  [-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1],
];

// Face definitions: vertex indices, theme token for the fill and the axis caption.
const RENDER_FACES = [
  { vertices: [0, 1, 2, 3], token: "--cube-face-zneg", alpha: 0.8, label: "Z−" },
  { vertices: [4, 5, 6, 7], token: "--cube-face-zpos", alpha: 0.78, label: "Z+" },
  { vertices: [0, 1, 5, 4], token: "--cube-face-yneg", alpha: 0.82, label: "Y−" },
  { vertices: [2, 3, 7, 6], token: "--cube-face-ypos", alpha: 0.82, label: "Y+" },
  { vertices: [1, 2, 6, 5], token: "--cube-face-xpos", alpha: 0.82, label: "X+" },
  { vertices: [0, 3, 7, 4], token: "--cube-face-xneg", alpha: 0.82, label: "X−" },
];

const HIT_TEST_FACES = [
  { vertices: [0, 1, 2, 3], axis: "Z", sign: -1 },
  { vertices: [4, 5, 6, 7], axis: "Z", sign: 1 },
  { vertices: [0, 1, 5, 4], axis: "Y", sign: -1 },
  { vertices: [2, 3, 7, 6], axis: "Y", sign: 1 },
  { vertices: [1, 2, 6, 5], axis: "X", sign: 1 },
  { vertices: [0, 3, 7, 4], axis: "X", sign: -1 },
];

function projectVertices(width, height, camera) {
  const centerX = width / 2;
  const centerY = height / 2;
  const size = Math.min(width, height) * 0.24;

  return VERTICES.map((vertex) => {
    const point = rotatePoint(vertex, camera.yaw, camera.pitch);
    return {
      x: centerX + point[0] * size,
      y: centerY - point[1] * size,
      depth: point[2],
    };
  });
}

/**
 * Draws the axis caption on one cube face, skewing the text to the face plane.
 */
function drawFaceLabel(context, points, label) {
  const p0 = points[0];
  const p1 = points[1];
  const p3 = points[3];
  const centerX = (p0.x + points[2].x) / 2;
  const centerY = (p0.y + points[2].y) / 2;

  context.save();
  context.translate(centerX, centerY);
  context.transform(
    (p1.x - p0.x) / 2,
    (p1.y - p0.y) / 2,
    (p3.x - p0.x) / 2,
    (p3.y - p0.y) / 2,
    0,
    0,
  );
  context.fillStyle = themeToken("--cube-label");
  context.font = "bold 0.58px system-ui";
  context.textAlign = "center";
  context.textBaseline = "middle";
  context.fillText(label, 0, 0);
  context.restore();
}

function pointInPolygon(x, y, points) {
  let inside = false;

  for (let i = 0, j = points.length - 1; i < points.length; j = i++) {
    const [xi, yi] = points[i];
    const [xj, yj] = points[j];
    const crossesRay = (yi > y) !== (yj > y);
    if (crossesRay && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) {
      inside = !inside;
    }
  }

  return inside;
}

export function initOrientationCube({ canvas, camera, onView }) {
  const context = canvas.getContext("2d");

  function render() {
    const { width, height } = resizeCanvas(canvas, context);
    context.clearRect(0, 0, width, height);
    const projected = projectVertices(width, height, camera);
    const edgeColor = themeAlpha("--cube-edge", 0.58);
    const faces = RENDER_FACES.map((face) => ({
      ...face,
      color: themeAlpha(face.token, face.alpha),
      depth: face.vertices.reduce((sum, index) => sum + projected[index].depth, 0) / 4,
    })).sort((a, b) => a.depth - b.depth);

    for (const face of faces) {
      const points = face.vertices.map((index) => projected[index]);
      context.beginPath();
      context.moveTo(points[0].x, points[0].y);
      points.slice(1).forEach((point) => context.lineTo(point.x, point.y));
      context.closePath();
      context.fillStyle = face.color;
      context.fill();
      context.strokeStyle = edgeColor;
      context.lineWidth = 1;
      context.stroke();
      drawFaceLabel(context, points, face.label);
    }
  }

  canvas.addEventListener("click", (event) => {
    const rect = canvas.getBoundingClientRect();
    const width = rect.width;
    const height = rect.height;
    const centerX = width / 2;
    const centerY = height / 2;
    const size = Math.min(width, height) * 0.24;
    const projected = VERTICES.map((vertex) => {
      const point = rotatePoint(vertex, camera.yaw, camera.pitch);
      return [centerX + point[0] * size, centerY - point[1] * size, point[2]];
    });
    const faces = HIT_TEST_FACES.map((face) => ({
      ...face,
      depth: face.vertices.reduce((sum, index) => sum + projected[index][2], 0) / 4,
    })).sort((a, b) => a.depth - b.depth);

    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;

    // Check front faces first, in back-to-front drawing order.
    for (const face of faces.reverse()) {
      const polygon = face.vertices.map((index) => projected[index]);
      if (pointInPolygon(x, y, polygon)) {
        onView(face.axis, face.sign);
        return;
      }
    }
  });

  return { render };
}
