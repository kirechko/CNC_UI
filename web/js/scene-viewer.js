import {
  calculateSceneBounds,
  calculateTrajectoryBounds,
  makeProjector,
} from "./scene-projection.js";
import { drawAxes, drawGrid } from "./scene-renderer.js";
import { drawStockBox, drawStockMesh } from "./stock-renderer.js";
import { drawTool as renderTool } from "./tool-renderer.js";
import { calculateToolpathProgress } from "./toolpath-progress.js";
import { drawTrajectorySegments } from "./trajectory-renderer.js";
import { resizeCanvas } from "./canvas-utils.js";

export function initSceneViewer({
  trajectoryCanvas,
  trajectoryContext,
  toolCanvas,
  toolContext,
  viewerEmpty,
  getProgram,
  getStockMesh,
  isStockVisible,
  getCamera,
  getStatus,
  getToolPosition,
  getCurrentProgramLine,
  isConnected,
  getToolRotationPhase,
  renderOrientationCube,
}) {
  let trackedSegments = null;
  let lastProgress = null;
  let lastPosition = null;
  let lastProgramLine = 0;

  function drawTool() {
    const camera = getCamera();
    const program = getProgram();
    const stockMesh = getStockMesh();
    const bounds = calculateSceneBounds(program?.segments || [], stockMesh);

    renderTool({
      canvas: toolCanvas,
      context: toolContext,
      connected: isConnected(),
      position: getToolPosition(),
      rotationPhase: getToolRotationPhase(),
      bounds,
      camera,
    });
  }

  function drawStock(context, project, bounds, angleA, angleB, stockMesh) {
    if (stockMesh) {
      drawStockMesh(context, project, stockMesh, angleA, angleB);
    } else {
      drawStockBox(context, project, bounds, angleA, angleB);
    }
  }

  function draw() {
    const { width, height } = resizeCanvas(trajectoryCanvas, trajectoryContext);
    const program = getProgram();
    const stockMesh = getStockMesh();
    const segments = program?.segments || [];
    const camera = getCamera();
    const bounds = calculateSceneBounds(segments, stockMesh);
    const project = makeProjector(width, height, bounds, camera);

    trajectoryContext.clearRect(0, 0, width, height);

    if (segments.length === 0 && !stockMesh) {
      viewerEmpty.hidden = false;
      renderOrientationCube();
      drawTool();
      return;
    }

    viewerEmpty.hidden = true;
    const stockBounds = stockMesh
      ? { min: stockMesh.min, max: stockMesh.max }
      : calculateTrajectoryBounds(segments, stockMesh);
    drawGrid(trajectoryContext, width, height, project, stockBounds);

    const status = getStatus();
    const angleA = (Number(status?.machine_a) || 0) * Math.PI / 180;
    const angleB = (Number(status?.machine_b) || 0) * Math.PI / 180;
    if (isStockVisible()) {
      drawStock(trajectoryContext, project, stockBounds, angleA, angleB, stockMesh);
    }

    const currentProgramLine = getCurrentProgramLine();
    const toolPosition = getToolPosition();

    if (
      trackedSegments !== segments ||
      currentProgramLine + 1 < lastProgramLine
    ) {
      trackedSegments = segments;
      lastProgress = null;
      lastPosition = null;
    }

    const displacement = lastPosition
      ? Math.hypot(
          toolPosition[0] - lastPosition[0],
          toolPosition[1] - lastPosition[1],
          toolPosition[2] - lastPosition[2],
        )
      : Infinity;
    const maxAdvance = Number.isFinite(displacement)
      ? displacement * 1.5 + 0.5
      : Infinity;
    const activeProgress = calculateToolpathProgress(
      segments,
      currentProgramLine,
      toolPosition,
      lastProgress,
      maxAdvance,
    );

    if (activeProgress) lastProgress = activeProgress;
    if (
      Array.isArray(toolPosition) &&
      toolPosition.length >= 3 &&
      toolPosition.slice(0, 3).every(Number.isFinite)
    ) {
      lastPosition = toolPosition.slice(0, 3);
    }
    lastProgramLine = currentProgramLine;

    drawTrajectorySegments({
      context: trajectoryContext,
      segments,
      currentProgramLine: activeProgress?.line ?? currentProgramLine,
      activeProgress,
      project,
    });

    drawAxes(trajectoryContext, project, bounds);
    renderOrientationCube();
    drawTool();
  }

  return { draw, drawTool };
}
