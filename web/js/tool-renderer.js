import { makeProjector } from "./scene-projection.js";
import { themeAlpha, themeToken } from "./theme-colors.js";

/**
 * Draws the demo cutter, a profile-selected primitive tool descriptor, or a laser nozzle.
 * Arbitrary uploaded meshes are not available in the current local tool schema.
 * @param {object} options - Canvas, machine pose, geometry descriptor and laser state.
 */
export function drawTool({
  canvas,
  context,
  connected,
  position,
  rotationPhase,
  bounds,
  camera,
  laserIntensity = 0,
  isLaserMachine = false,
  toolGeometry = null,
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
  const sceneSpan = Math.max(bounds.max[0] - bounds.min[0], bounds.max[1] - bounds.min[1], bounds.max[2] - bounds.min[2], 10);
  const fallbackRadius = Math.max(0.8, sceneSpan * 0.008);
  const geometry = toolGeometry && typeof toolGeometry === "object" ? toolGeometry : null;
  const geometryName = typeof toolGeometry === "string" ? toolGeometry : geometry?.geometryModel;
  const diameter = Number(geometry?.diameter);
  const length = Number(geometry?.length);
  const radius = Number.isFinite(diameter) && diameter > 0 ? diameter / 2 : fallbackRadius;
  const bodyLength = Number.isFinite(length) && length > 0 ? length : Math.max(12, sceneSpan * 0.12);
  const tip = [...position];
  const pointLength = Math.max(3, radius * 2.2);
  const lowerZ = tip[2] + (isLaserMachine ? Math.max(2, radius * 0.65) : pointLength);
  const upperZ = lowerZ + bodyLength;
  const sideCount = geometryName ? 12 : 6;
  const ring = (z, ringRadius = radius) => Array.from({ length: sideCount }, (_, index) => {
    const angle = rotationPhase + index * Math.PI * 2 / sideCount;
    return project([tip[0] + ringRadius * Math.cos(angle), tip[1] + ringRadius * Math.sin(angle), z]);
  });
  const lower = ring(lowerZ);
  const upper = ring(upperZ);
  const projectedTip = project(tip);
  const capCenter = project([tip[0], tip[1], upperZ]);
  const faces = Array.from({ length: sideCount }, (_, index) => {
    const next = (index + 1) % sideCount;
    const color = geometryName ? (index % 2 ? "#96a1ad" : "#d5dbe2") : themeToken([
      "--tool-face-mid", "--tool-face-shade", "--tool-face-light", "--tool-face-shade", "--tool-face-mid", "--tool-face-dark",
    ][index % 6]);
    return { points: [lower[index], lower[next], upper[next], upper[index]], color,
      depth: (lower[index].depth + lower[next].depth + upper[index].depth + upper[next].depth) / 4 };
  }).sort((a, b) => a.depth - b.depth);

  // Draw the shank as depth-sorted facets; fixed vertical stripes reveal rotation of library geometry.
  for (const face of faces) {
    context.beginPath();
    context.moveTo(face.points[0].x, face.points[0].y);
    face.points.slice(1).forEach((point) => context.lineTo(point.x, point.y));
    context.closePath();
    context.fillStyle = isLaserMachine ? "#aeb8c4" : face.color;
    context.fill();
    context.strokeStyle = isLaserMachine ? "#58616a" : themeAlpha("--tool-edge", 0.75);
    context.lineWidth = 1;
    context.stroke();
  }

  if (isLaserMachine) {
    // Build the nozzle as individually shaded triangles so each facet has a consistent outward normal.
    const nozzleTip = [...tip];
    const projectedNozzleTip = project(nozzleTip);
    const coneFaces = Array.from({ length: sideCount }, (_, index) => {
      const next = (index + 1) % sideCount;
      const angle = (side) => rotationPhase + side * Math.PI * 2 / sideCount;
      const basePoint = (side) => [
        tip[0] + radius * Math.cos(angle(side)),
        tip[1] + radius * Math.sin(angle(side)),
        lowerZ,
      ];
      const current = basePoint(index);
      const following = basePoint(next);
      const edgeA = following.map((value, axis) => value - nozzleTip[axis]);
      const edgeB = current.map((value, axis) => value - nozzleTip[axis]);
      // This vertex order makes edgeA × edgeB point radially outward from the cone.
      const normal = [
        edgeA[1] * edgeB[2] - edgeA[2] * edgeB[1],
        edgeA[2] * edgeB[0] - edgeA[0] * edgeB[2],
        edgeA[0] * edgeB[1] - edgeA[1] * edgeB[0],
      ];
      const normalLength = Math.hypot(...normal) || 1;
      const light = [-0.45, -0.6, 0.66];
      const lightLength = Math.hypot(...light);
      const diffuse = Math.max(0, normal.reduce((sum, value, axis) => sum + value / normalLength * light[axis] / lightLength, 0));
      const shade = Math.round(91 + diffuse * 78);
      const projectedCurrent = project(current);
      const projectedFollowing = project(following);
      return {
        points: [projectedNozzleTip, projectedFollowing, projectedCurrent],
        color: `rgb(${shade}, ${shade + 5}, ${shade + 11})`,
        depth: (projectedNozzleTip.depth + projectedCurrent.depth + projectedFollowing.depth) / 3,
      };
    }).sort((a, b) => a.depth - b.depth);

    for (const face of coneFaces) {
      context.beginPath();
      context.moveTo(face.points[0].x, face.points[0].y);
      context.lineTo(face.points[1].x, face.points[1].y);
      context.lineTo(face.points[2].x, face.points[2].y);
      context.closePath();
      context.fillStyle = face.color;
      context.fill();
      context.strokeStyle = "#444c54";
      context.lineWidth = 0.7;
      context.stroke();
    }
  } else {
    const tipScale = geometryName === "ball_nose" ? 0.38 : ["tapered", "v_bit", "drill"].includes(geometryName) ? 0.72 : 1;
    const cuttingRing = ring(tip[2] + pointLength * tipScale, radius * tipScale);
    for (let index = 0; index < sideCount; index++) {
      const next = (index + 1) % sideCount;
      context.beginPath();
      context.moveTo(projectedTip.x, projectedTip.y);
      context.lineTo(cuttingRing[index].x, cuttingRing[index].y);
      context.lineTo(cuttingRing[next].x, cuttingRing[next].y);
      context.closePath();
      context.fillStyle = geometryName ? (index % 2 ? "#8e9aa8" : "#d9e1ea") : (index % 2 ? themeToken("--tool-wood-dark") : themeToken("--tool-wood-light"));
      context.fill();
      context.strokeStyle = geometryName ? "#58616a" : themeAlpha("--tool-wood-edge", 0.62);
      context.lineWidth = 0.7;
      context.stroke();
    }
    if (geometryName) {
      context.save();
      context.strokeStyle = "rgba(25, 30, 38, 0.78)";
      context.lineWidth = Math.max(1, radius * 0.14);
      for (let index = 0; index < 3; index++) {
        const angle = rotationPhase + index * Math.PI * 2 / 3;
        const start = project([tip[0] + radius * Math.cos(angle), tip[1] + radius * Math.sin(angle), lowerZ]);
        const end = project([tip[0] + radius * Math.cos(angle), tip[1] + radius * Math.sin(angle), upperZ]);
        context.beginPath(); context.moveTo(start.x, start.y); context.lineTo(end.x, end.y); context.stroke();
      }
      context.restore();
    }
  }

  // Beam is emitted only when the caller supplies a validated, positive laser intensity.
  if (isLaserMachine && Number.isFinite(laserIntensity) && laserIntensity > 0) {
    const intensity = Math.min(1, laserIntensity);
    const beamEnd = project([tip[0], tip[1], tip[2] - Math.max(4, sceneSpan * 0.08)]);
    const beamWidth = Math.max(1, radius * 0.65) * (0.35 + intensity * 0.65);
    context.save();
    context.globalAlpha = 0.18 + intensity * 0.78;
    context.shadowColor = "#ff2020";
    context.shadowBlur = 3 + intensity * 12;
    context.fillStyle = "#ff2020";
    context.beginPath();
    context.moveTo(projectedTip.x - beamWidth, projectedTip.y);
    context.lineTo(projectedTip.x + beamWidth, projectedTip.y);
    context.lineTo(beamEnd.x, beamEnd.y);
    context.closePath();
    context.fill();
    context.restore();
  }
}
