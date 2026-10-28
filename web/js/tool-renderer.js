import { makeProjector } from "./scene-projection.js";
import { themeAlpha, themeToken } from "./theme-colors.js";

/**
 * Renders the rotating tool (a hexagonal pencil) at the machine position.
 * Body, sharpened wood, cap and graphite tip colors come from theme tokens so
 * the tool blends with whichever UI theme is active.
 */
export function drawTool({
  canvas,
  context,
  connected,
  position,
  rotationPhase,
  bounds,
  camera,
}) {
  const rect = canvas.getBoundingClientRect();
  const ratio = window.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(rect.width * ratio));
  const height = Math.max(1, Math.round(rect.height * ratio));

  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
  }

  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  context.clearRect(0, 0, rect.width, rect.height);
  if (!connected) return;

  const project = makeProjector(rect.width, rect.height, bounds, camera);
  const sceneSpan = Math.max(
    bounds.max[0] - bounds.min[0],
    bounds.max[1] - bounds.min[1],
    bounds.max[2] - bounds.min[2],
    10,
  );
  const radius = Math.max(0.8, sceneSpan * 0.008);
  const pointLength = Math.max(3, radius * 2.2);
  const bodyLength = Math.max(12, sceneSpan * 0.12);
  const tip = [...position];
  const lowerZ = tip[2] + pointLength;
  const upperZ = lowerZ + bodyLength;

  const ring = (z) => Array.from({ length: 6 }, (_, index) => {
    const angle = rotationPhase + index * Math.PI / 3;
    return project([
      tip[0] + radius * Math.cos(angle),
      tip[1] + radius * Math.sin(angle),
      z,
    ]);
  });

  const lower = ring(lowerZ);
  const upper = ring(upperZ);
  const projectedTip = project(tip);
  // Six facets alternate light/shade tones from the theme for a metallic look.
  const faceTokens = [
    "--tool-face-mid", "--tool-face-shade", "--tool-face-light",
    "--tool-face-shade", "--tool-face-mid", "--tool-face-dark",
  ];
  const faces = Array.from({ length: 6 }, (_, index) => {
    const next = (index + 1) % 6;
    return {
      points: [lower[index], lower[next], upper[next], upper[index]],
      depth:
        (lower[index].depth + lower[next].depth + upper[next].depth + upper[index].depth) / 4,
      color: themeToken(faceTokens[index]),
    };
  }).sort((a, b) => a.depth - b.depth);

  // Faceted body of the rotating hexagonal pencil.
  for (const face of faces) {
    context.beginPath();
    context.moveTo(face.points[0].x, face.points[0].y);
    face.points.slice(1).forEach((point) => context.lineTo(point.x, point.y));
    context.closePath();
    context.fillStyle = face.color;
    context.fill();
    context.strokeStyle = themeAlpha("--tool-edge", 0.75);
    context.lineWidth = 1;
    context.stroke();
  }

  // Sharpened wooden section and graphite tip.
  for (let index = 0; index < 6; index++) {
    const next = (index + 1) % 6;
    context.beginPath();
    context.moveTo(projectedTip.x, projectedTip.y);
    context.lineTo(lower[index].x, lower[index].y);
    context.lineTo(lower[next].x, lower[next].y);
    context.closePath();
    context.fillStyle = index % 2 === 0
      ? themeToken("--tool-wood-light")
      : themeToken("--tool-wood-dark");
    context.fill();
    context.strokeStyle = themeAlpha("--tool-wood-edge", 0.62);
    context.lineWidth = 0.7;
    context.stroke();
  }

  const capCenter = project([tip[0], tip[1], upperZ]);
  context.beginPath();
  upper.forEach((point, index) => {
    if (index === 0) context.moveTo(point.x, point.y);
    else context.lineTo(point.x, point.y);
  });
  context.closePath();
  context.fillStyle = themeToken("--tool-cap");
  context.fill();
  context.strokeStyle = themeAlpha("--tool-cap-edge", 0.8);
  context.lineWidth = 1;
  context.stroke();

  context.beginPath();
  context.moveTo(projectedTip.x, projectedTip.y);
  context.lineTo(capCenter.x, capCenter.y);
  context.strokeStyle = themeToken("--tool-tip-line");
  context.lineWidth = 1.2;
  context.stroke();
}
