import { resizeCanvas } from "./canvas-utils.js";
import { themeAlpha, themeToken } from "./theme-colors.js";

/**
 * Paints the circular JOG dial (four step rings around eight directions, or a
 * single solid ring in continuous mode). All sector colors come from theme
 * tokens (web/css/theme-*.css) so the dial follows the active UI theme.
 *
 * Sector separation (task): neighbors are drawn as FULL adjacent octants and
 * separated by the per-sector stroke — a hairline of constant pixel width
 * (`GAP_LINE_PX`) along each radial boundary. The previous design shrank every
 * sector by a fixed angle (+/- 0.012 rad), which left an empty wedge that grew
 * linearly with the radius: near the rim the gap was a wide background-colored
 * slice, in the hub it vanished. A stroked boundary keeps the same thin line
 * at every radius.
 */
const GAP_LINE_PX = 1.5;
/** Angular half-width of one of the eight dial sectors. */
const SECTOR_HALF = Math.PI / 8;
export function renderJogDial({
  canvas,
  context,
  mode,
  stepValues,
  hoveredSelection,
}) {
  const { width: w, height: h } = resizeCanvas(canvas, context);
  if (!w || !h) return;
  const cx = w / 2;
  const cy = h / 2;
  const radius = Math.min(w, h) * 0.46;
  const inner = [0.15, 0.3625, 0.575, 0.7875, 1];
  // Ring gradients per direction family, read from the current theme.
  const colorsX = ["--dial-x-1", "--dial-x-2", "--dial-x-3", "--dial-x-4"].map((token) => themeToken(token));
  const colorsY = ["--dial-y-1", "--dial-y-2", "--dial-y-3", "--dial-y-4"].map((token) => themeToken(token));
  const colorsDiagonal = ["--dial-diag-1", "--dial-diag-2", "--dial-diag-3", "--dial-diag-4"].map((token) => themeToken(token));
  // Stroke/label palette shared by both modes.
  const dialStroke = themeToken("--dial-stroke");
  const labelColor = themeToken("--dial-label");
  const hoverFill = themeAlpha("--dial-hover-fill", 0.24);
  const hoverStroke = themeAlpha("--dial-hover-stroke", 0.9);
  const labels = ["+X", "", "−Y", "", "−X", "", "+Y", ""];

  context.clearRect(0, 0, w, h);

  if (mode === "continuous") {
    // Solid directions reuse the darkest ring of each family (X red, diagonals blue, Y green).
    const directionColors = [
      colorsX[3], colorsDiagonal[3], colorsY[3], colorsDiagonal[3],
      colorsX[3], colorsDiagonal[3], colorsY[3], colorsDiagonal[3],
    ];

    for (let sector = 0; sector < 8; sector++) {
      const center = sector * Math.PI / 4;
      // Full octant: shared edges, no angular inset (constant-width separators).
      const start = center - SECTOR_HALF;
      const end = center + SECTOR_HALF;

      context.beginPath();
      context.moveTo(cx, cy);
      context.arc(cx, cy, radius, start, end);
      context.closePath();
      context.fillStyle = directionColors[sector];
      context.fill();
      context.strokeStyle = dialStroke;
      context.lineWidth = GAP_LINE_PX;
      context.stroke();

      if (hoveredSelection?.sector === sector) {
        context.beginPath();
        context.moveTo(cx, cy);
        context.arc(cx, cy, radius, start, end);
        context.closePath();
        context.fillStyle = hoverFill;
        context.fill();
        context.strokeStyle = hoverStroke;
        context.lineWidth = 2;
        context.stroke();
      }
    }

    const continuousLabels = ["+X", "", "−Y", "", "−X", "", "+Y", ""];
    for (let sector = 0; sector < continuousLabels.length; sector++) {
      const label = continuousLabels[sector];
      if (!label) continue;

      const angle = sector * Math.PI / 4;
      const labelRadius = radius * 0.76;
      context.fillStyle = labelColor;
      context.font = `bold ${Math.max(9, radius * 0.105)}px system-ui`;
      context.textAlign = "center";
      context.textBaseline = "middle";
      context.fillText(
        label,
        cx + Math.cos(angle) * labelRadius,
        cy + Math.sin(angle) * labelRadius,
      );
    }

    drawDialCenter(context, cx, cy, radius);
    return;
  }

  for (let ring = 0; ring < 4; ring++) {
    for (let sector = 0; sector < 8; sector++) {
      const center = sector * Math.PI / 4;
      // Full octants again: the boundary stroke is the constant-width gap line.
      const start = center - SECTOR_HALF;
      const end = center + SECTOR_HALF;
      const innerRadius = radius * inner[ring];
      const outerRadius = radius * inner[ring + 1];
      const color = sector % 4 === 0
        ? colorsX[ring]
        : sector % 4 === 2
          ? colorsY[ring]
          : colorsDiagonal[ring];

      context.beginPath();
      context.arc(cx, cy, outerRadius, start, end);
      context.arc(cx, cy, innerRadius, end, start, true);
      context.closePath();
      context.fillStyle = color;
      context.fill();
      context.strokeStyle = dialStroke;
      context.lineWidth = GAP_LINE_PX;
      context.stroke();

      if (labels[sector] && ring === 3) {
        const labelRadius = (innerRadius + outerRadius) / 2;
        context.fillStyle = labelColor;
        context.font = `bold ${Math.max(9, radius * 0.105)}px system-ui`;
        context.textAlign = "center";
        context.textBaseline = "middle";
        context.fillText(
          labels[sector],
          cx + Math.cos(center) * labelRadius,
          cy + Math.sin(center) * labelRadius,
        );
      }
    }
  }

  // Label the step size in each ring within the +X/+Y sector.
  const formatStep = (value) => String(value);
  const stepLabels = stepValues.map(formatStep);
  const labelAngle = 7 * Math.PI / 4;
  context.fillStyle = labelColor;
  context.font = `bold ${Math.max(8, radius * 0.09)}px system-ui`;
  context.textAlign = "center";
  context.textBaseline = "middle";
  for (let ring = 0; ring < stepLabels.length; ring++) {
    const labelRadius = radius * (inner[ring] + inner[ring + 1]) / 2;
    context.fillText(
      stepLabels[ring],
      cx + Math.cos(labelAngle) * labelRadius,
      cy + Math.sin(labelAngle) * labelRadius,
    );
  }

  if (hoveredSelection) {
    const ring = stepValues.indexOf(hoveredSelection.step);
    if (ring !== -1) {
      const sector = hoveredSelection.sector;
      const center = sector * Math.PI / 4;
      const start = center - SECTOR_HALF;
      const end = center + SECTOR_HALF;
      context.beginPath();
      context.arc(cx, cy, radius * inner[ring + 1], start, end);
      context.arc(cx, cy, radius * inner[ring], end, start, true);
      context.closePath();
      context.fillStyle = hoverFill;
      context.fill();
      context.strokeStyle = hoverStroke;
      context.lineWidth = 2;
      context.stroke();
    }
  }

  drawDialCenter(context, cx, cy, radius);
}

/**
 * Draws the small hub at the dial center: background disc, border ring and
 * a crosshair. Colors are theme tokens so the hub blends with the panels.
 */
function drawDialCenter(context, cx, cy, radius) {
  context.beginPath();
  context.arc(cx, cy, radius * 0.14, 0, Math.PI * 2);
  context.fillStyle = themeToken("--dial-center-bg");
  context.fill();
  context.strokeStyle = themeToken("--dial-center-border");
  context.lineWidth = 2;
  context.stroke();

  context.beginPath();
  context.moveTo(cx - 5, cy);
  context.lineTo(cx + 5, cy);
  context.moveTo(cx, cy - 5);
  context.lineTo(cx, cy + 5);
  context.strokeStyle = themeToken("--dial-center-cross");
  context.lineWidth = 1.5;
  context.stroke();
}
