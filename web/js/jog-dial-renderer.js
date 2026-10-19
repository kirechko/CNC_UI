import { resizeCanvas } from "./canvas-utils.js";

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
  const colorsX = ["#3f2a2b", "#532e2e", "#6e3332", "#8d3837"];
  const colorsY = ["#2b3937", "#314945", "#375e56", "#3d766c"];
  const colorsDiagonal = ["#314965", "#46668b", "#4f749e", "#5a85b6"];
  const labels = ["+X", "", "−Y", "", "−X", "", "+Y", ""];

  context.clearRect(0, 0, w, h);

  if (mode === "continuous") {
    const directionColors = [
      "#8d3837", "#5a85b6", "#3d766c", "#5a85b6",
      "#8d3837", "#5a85b6", "#3d766c", "#5a85b6",
    ];

    for (let sector = 0; sector < 8; sector++) {
      const center = sector * Math.PI / 4;
      const start = center - Math.PI / 8 + 0.012;
      const end = center + Math.PI / 8 - 0.012;

      context.beginPath();
      context.moveTo(cx, cy);
      context.arc(cx, cy, radius, start, end);
      context.closePath();
      context.fillStyle = directionColors[sector];
      context.fill();
      context.strokeStyle = "#111719";
      context.lineWidth = 1.5;
      context.stroke();

      if (hoveredSelection?.sector === sector) {
        context.beginPath();
        context.moveTo(cx, cy);
        context.arc(cx, cy, radius, start, end);
        context.closePath();
        context.fillStyle = "rgba(255, 255, 255, .24)";
        context.fill();
        context.strokeStyle = "rgba(255, 255, 255, .9)";
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
      context.fillStyle = "#f1f3f3";
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
      const start = center - Math.PI / 8 + 0.012;
      const end = center + Math.PI / 8 - 0.012;
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
      context.strokeStyle = "#111719";
      context.lineWidth = 1.5;
      context.stroke();

      if (labels[sector] && ring === 3) {
        const labelRadius = (innerRadius + outerRadius) / 2;
        context.fillStyle = "#f1f3f3";
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

  // Подписываем размер шага в каждом кольце в секторе +X/+Y.
  const formatStep = (value) => String(value);
  const stepLabels = stepValues.map(formatStep);
  const labelAngle = 7 * Math.PI / 4;
  context.fillStyle = "#f1f3f3";
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
      const start = center - Math.PI / 8 + 0.012;
      const end = center + Math.PI / 8 - 0.012;
      context.beginPath();
      context.arc(cx, cy, radius * inner[ring + 1], start, end);
      context.arc(cx, cy, radius * inner[ring], end, start, true);
      context.closePath();
      context.fillStyle = "rgba(255, 255, 255, .24)";
      context.fill();
      context.strokeStyle = "rgba(255, 255, 255, .9)";
      context.lineWidth = 2;
      context.stroke();
    }
  }

  drawDialCenter(context, cx, cy, radius);
}

function drawDialCenter(context, cx, cy, radius) {
  context.beginPath();
  context.arc(cx, cy, radius * 0.14, 0, Math.PI * 2);
  context.fillStyle = "#202629";
  context.fill();
  context.strokeStyle = "#101416";
  context.lineWidth = 2;
  context.stroke();

  context.beginPath();
  context.moveTo(cx - 5, cy);
  context.lineTo(cx + 5, cy);
  context.moveTo(cx, cy - 5);
  context.lineTo(cx, cy + 5);
  context.strokeStyle = "#879398";
  context.lineWidth = 1.5;
  context.stroke();
}
