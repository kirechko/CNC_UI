import { rotatePoint } from "./scene-projection.js";

const VERTICES = [
  [-1, -1, -1], [1, -1, -1], [1, 1, -1], [-1, 1, -1],
  [-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1],
];

const RENDER_FACES = [
  { vertices: [0, 1, 2, 3], color: "rgba(74, 83, 88, .8)", label: "Z−" },
  { vertices: [4, 5, 6, 7], color: "rgba(40, 126, 170, .78)", label: "Z+" },
  { vertices: [0, 1, 5, 4], color: "rgba(151, 74, 77, .82)", label: "Y−" },
  { vertices: [2, 3, 7, 6], color: "rgba(67, 139, 103, .82)", label: "Y+" },
  { vertices: [1, 2, 6, 5], color: "rgba(166, 83, 83, .82)", label: "X+" },
  { vertices: [0, 3, 7, 4], color: "rgba(87, 132, 102, .82)", label: "X−" },
];

const HIT_TEST_FACES = [
  { vertices: [0, 1, 2, 3], axis: "Z", sign: -1 },
  { vertices: [4, 5, 6, 7], axis: "Z", sign: 1 },
  { vertices: [0, 1, 5, 4], axis: "Y", sign: -1 },
  { vertices: [2, 3, 7, 6], axis: "Y", sign: 1 },
  { vertices: [1, 2, 6, 5], axis: "X", sign: 1 },
  { vertices: [0, 3, 7, 4], axis: "X", sign: -1 },
];

function resizeCanvas(canvas, context) {
  const rect = canvas.getBoundingClientRect();
  const ratio = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(rect.width * ratio));
  const height = Math.max(1, Math.round(rect.height * ratio));

  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }

  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  return { width: rect.width, height: rect.height };
}

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
  context.fillStyle = "#f0f4f5";
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
    const faces = RENDER_FACES.map((face) => ({
      ...face,
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
      context.strokeStyle = "rgba(206, 220, 225, .58)";
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

    // Сначала проверяем передние грани — в порядке отрисовки back-to-front.
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
