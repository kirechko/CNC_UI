import { themeAlpha, themeToken } from "./theme-colors.js";

/**
 * Fills the XY work-plane with a translucent sheet and draws the minor/major
 * background grid plus the work-zero origin dot. All colors come from theme
 * tokens so the scene follows the active UI theme.
 */
export function drawGrid(context, width, height, project, bounds) {
  // The XY plane is twice the stock size and centered on its bounds.
  const stockWidth = bounds.max[0] - bounds.min[0];
  const stockHeight = bounds.max[1] - bounds.min[1];
  const planeMinX = bounds.min[0] - stockWidth / 2;
  const planeMaxX = bounds.max[0] + stockWidth / 2;
  const planeMinY = bounds.min[1] - stockHeight / 2;
  const planeMaxY = bounds.max[1] + stockHeight / 2;
  const corners = [
    [planeMinX, planeMinY, 0],
    [planeMaxX, planeMinY, 0],
    [planeMaxX, planeMaxY, 0],
    [planeMinX, planeMaxY, 0],
  ].map(project);
  context.beginPath();
  corners.forEach((point, index) => {
    if (index === 0) context.moveTo(point.x, point.y);
    else context.lineTo(point.x, point.y);
  });
  context.closePath();
  context.fillStyle = themeAlpha("--viewer-grid-fill", 0.52);
  context.fill();
  context.strokeStyle = themeAlpha("--viewer-grid-border", 0.38);
  context.lineWidth = 1;
  context.stroke();

  const rangeX = planeMaxX - planeMinX;
  const rangeY = planeMaxY - planeMinY;
  const step = Math.pow(10, Math.floor(Math.log10(Math.max(rangeX, rangeY) / 8)));
  const minorStep = step / 5;
  context.lineWidth = 1;

  for (let x = Math.ceil(planeMinX / minorStep) * minorStep; x <= planeMaxX; x += minorStep) {
    const a = project([x, planeMinY, 0]);
    const b = project([x, planeMaxY, 0]);
    context.beginPath();
    context.moveTo(a.x, a.y);
    context.lineTo(b.x, b.y);
    const major = Math.abs(x / step - Math.round(x / step)) < 1e-6;
    context.strokeStyle = major
      ? themeAlpha("--viewer-grid-line", 0.3)
      : themeAlpha("--viewer-grid-line", 0.13);
    context.stroke();
  }

  for (let y = Math.ceil(planeMinY / minorStep) * minorStep; y <= planeMaxY; y += minorStep) {
    const a = project([planeMinX, y, 0]);
    const b = project([planeMaxX, y, 0]);
    context.beginPath();
    context.moveTo(a.x, a.y);
    context.lineTo(b.x, b.y);
    const major = Math.abs(y / step - Math.round(y / step)) < 1e-6;
    context.strokeStyle = major
      ? themeAlpha("--viewer-grid-line", 0.3)
      : themeAlpha("--viewer-grid-line", 0.13);
    context.stroke();
  }

  const origin = project([0, 0, 0]);
  context.fillStyle = themeToken("--axis-x");
  context.beginPath();
  context.arc(origin.x, origin.y, 3, 0, Math.PI * 2);
  context.fill();
}

/**
 * Draws the colored X/Y/Z axis triad at the work zero. Axis colors reuse the
 * `--axis-*` theme tokens shared with the legend and the rotary buttons.
 */
export function drawAxes(context, project, bounds) {
  const length = Math.max(
    bounds.max[0] - bounds.min[0],
    bounds.max[1] - bounds.min[1],
    bounds.max[2] - bounds.min[2],
    10,
  ) * 0.14;
  const origin = project([0, 0, 0]);

  for (const [axis, end, color] of [
    ["X", [length, 0, 0], themeToken("--axis-x")],
    ["Y", [0, length, 0], themeToken("--axis-y")],
    ["Z", [0, 0, length], themeToken("--axis-z")],
  ]) {
    const point = project(end);
    context.beginPath();
    context.moveTo(origin.x, origin.y);
    context.lineTo(point.x, point.y);
    context.strokeStyle = color;
    context.lineWidth = 2;
    context.stroke();
    context.fillStyle = color;
    context.font = "bold 11px system-ui";
    context.fillText(axis, point.x + 4, point.y - 4);
  }
}
