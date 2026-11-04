import {
  calculateSceneBounds,
  calculateTrajectoryBounds,
  makeProjector,
} from "./scene-projection.js";
import { drawAxes, drawGrid } from "./scene-renderer.js";
import { drawStockBox, drawStockMesh } from "./stock-renderer.js";
import { drawTool as renderTool } from "./tool-renderer.js";
import { drawTrajectorySegments } from "./trajectory-renderer.js";
import { resizeCanvas } from "./canvas-utils.js";

/**
 * Creates the scene viewer that draws the trajectory, stock, axes and tool.
 *
 * Trajectory progress is taken exclusively from the backend status
 * (`progress.path_distance_mm`); the viewer does not infer progress itself.
 *
 * @param {object} options - Viewer dependencies and DOM/canvas handles.
 * @returns {{draw: () => void, drawTool: () => void}} Drawing entry points.
 */
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
  isConnected,
  getToolRotationPhase,
  getLaserIntensity,
  getMachineProfile,
  getTools,
  renderOrientationCube,
}) {
  /**
   * Draws the tool marker on its own canvas.
   */
  function drawTool() {
    const camera = getCamera();
    const program = getProgram();
    const stockMesh = getStockMesh();
    const bounds = calculateSceneBounds(program?.segments || [], stockMesh);
    const machineProfile = getMachineProfile?.();
    const machineType = machineProfile?.machineType;
    const isLaserMachine = machineType === "laser"
      || (machineType !== "mill" && getStatus()?.laser_mode_available === true);
    const selectedTool = machineProfile?.toolReference
      ? getTools?.().find((tool) => tool.id === machineProfile.toolReference) ?? null
      : null;
    const toolGeometry = selectedTool?.geometry ?? selectedTool?.geometryModel ?? null;

    renderTool({
      canvas: toolCanvas,
      context: toolContext,
      connected: isConnected(),
      position: getToolPosition(),
      rotationPhase: getToolRotationPhase(),
      bounds,
      camera,
      laserIntensity: getLaserIntensity(),
      isLaserMachine,
      toolGeometry,
    });
  }

  /**
   * Draws the stock as a mesh when available, otherwise as a bounding box.
   *
   * @param {CanvasRenderingContext2D} context - Target 2D context.
   * @param {(point: number[]) => {x: number, y: number}} project - Projection function.
   * @param {{min: number[], max: number[]}} bounds - Stock bounds.
   * @param {number} angleA - Machine A rotation in radians.
   * @param {number} angleB - Machine B rotation in radians.
   * @param {object|null} stockMesh - Loaded stock mesh, if any.
   */
  function drawStock(context, project, bounds, angleA, angleB, stockMesh) {
    if (stockMesh) {
      drawStockMesh(context, project, stockMesh, angleA, angleB);
    } else {
      drawStockBox(context, project, bounds, angleA, angleB);
    }
  }

  /**
   * Redraws the whole trajectory canvas and the tool canvas.
   */
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

    const profileType = getMachineProfile?.()?.machineType;
    const laserMode = profileType === "laser"
      || (profileType === "hybrid" && (status?.spindle_mode === "laser_constant" || status?.spindle_mode === "laser_dynamic"))
      || (profileType == null && (status?.spindle_mode === "laser_constant" || status?.spindle_mode === "laser_dynamic"));
    drawTrajectorySegments({
      context: trajectoryContext,
      segments,
      progress: status?.progress ?? null,
      project,
      laserMode,
      laserPowerMaximum: status?.configuration?.spindle?.laser?.max_power ?? status?.spindle_max_rpm,
    });

    drawAxes(trajectoryContext, project, bounds);
    document.querySelector("#viewer-stage")?.classList.toggle("laser-mode", laserMode);
    const laserLegend = document.querySelector(".viewer-legend-laser");
    if (laserLegend) laserLegend.hidden = !laserMode;
    renderOrientationCube();
    drawTool();
  }

  return { draw, drawTool };
}
