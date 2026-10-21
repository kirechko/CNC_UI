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

    const activeProgress = calculateToolpathProgress(
      segments,
      getCurrentProgramLine(),
      getToolPosition(),
    );
    drawTrajectorySegments({
      context: trajectoryContext,
      segments,
      currentProgramLine: activeProgress?.line ?? getCurrentProgramLine(),
      activeProgress,
      project,
    });

    drawAxes(trajectoryContext, project, bounds);
    renderOrientationCube();
    drawTool();
  }

  return { draw, drawTool };
}
