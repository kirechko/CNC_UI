// 3D viewer stage controller: owns the scene camera, the tool position
// vector, the spindle rotation animation, the stock mesh visibility, the
// orientation cube and the viewer toolbar. Extracted verbatim from main.js;
// cross-feature reads (status, connection, program) go through the shared
// `app` bag so this module never touches another feature's private state.
import { t } from "./i18n.js";
import { initSceneViewer } from "./scene-viewer.js";
import { initToolAnimation } from "./tool-animation.js";
import { initOrientationCube } from "./orientation-cube.js";
import { initCameraControls } from "./camera-controls.js";
import { initStockLoader } from "./stock-loader.js";
import { initViewerToolbar } from "./viewer-toolbar.js";

/**
 * Creates the viewer stage and all of its sub-controllers.
 *
 * @param {object} app - Shared application bag exposing `getStatus()`,
 *   `getConnected()` and `getProgram()`.
 * @returns {object} Scene handle: `camera`, `toolPosition`, `draw`,
 *   `drawTool`, `viewerToolbar`, `applyStatus(status)`, `clearTool()`.
 */
export function initSceneStage(app) {
  const trajectoryCanvas = document.querySelector("#trajectory-canvas");
  const trajectoryContext = trajectoryCanvas.getContext("2d");
  const toolCanvas = document.querySelector("#tool-canvas");
  const toolContext = toolCanvas.getContext("2d");
  const orientationCube = document.querySelector("#orientation-cube");
  const viewerEmpty = document.querySelector("#viewer-empty");
  const programName = document.querySelector("#program-name");
  const camera = { yaw: -0.72, pitch: 0.62, zoom: 1, panX: 0, panY: 0, perspective: true };
  const toolPosition = [0, 0, 0];
  let toolSpindleRpm = 0;
  let toolRotationPhase = 0;

  // Stock visibility and the loaded 3D mesh are private to the stage; the
  // stock toggle button and file loader below mutate them.
  let loadedStockMesh = null;
  let stockVisible = true;

  const sceneViewer = initSceneViewer({
    trajectoryCanvas,
    trajectoryContext,
    toolCanvas,
    toolContext,
    viewerEmpty,
    getProgram: () => app.getProgram(),
    getStockMesh: () => loadedStockMesh,
    isStockVisible: () => stockVisible,
    getCamera: () => camera,
    getStatus: () => app.getStatus(),
    getToolPosition: () => toolPosition,
    isConnected: () => app.getConnected(),
    getToolRotationPhase: () => toolRotationPhase,
    renderOrientationCube: () => orientationCubeRenderer.render(),
  });

  function drawToolpath() {
    sceneViewer.draw();
  }

  const toolAnimation = initToolAnimation({
    isActive: () => app.getConnected() && toolSpindleRpm > 0,
    advance: (elapsed) => {
      // Scale the real RPM for readable animation while keeping RPM proportional.
      toolRotationPhase = (
        toolRotationPhase + toolSpindleRpm * (Math.PI * 2 / 60) * 0.05 * elapsed
      ) % (Math.PI * 2);
    },
    render: sceneViewer.drawTool,
  });

  const orientationCubeRenderer = initOrientationCube({
    canvas: orientationCube,
    camera,
    onView: setView,
  });

  function setView(axis, sign = 1) {
    if (axis === "Z") {
      camera.yaw = -0.72;
      camera.pitch = sign > 0 ? Math.PI / 2 - 0.001 : -Math.PI / 2 + 0.001;
    } else if (axis === "X") {
      camera.yaw = sign > 0 ? Math.PI / 2 : -Math.PI / 2;
      camera.pitch = 0.001;
    } else {
      camera.yaw = sign > 0 ? 0 : Math.PI;
      camera.pitch = 0.001;
    }
    camera.panX = 0;
    camera.panY = 0;
    drawToolpath();
  }

  initCameraControls({
    canvas: trajectoryCanvas,
    camera,
    render: drawToolpath,
  });

  const stockToggle = document.querySelector("#stock-toggle");

  stockToggle.addEventListener("click", () => {
    stockVisible = !stockVisible;
    stockToggle.classList.toggle("active", stockVisible);
    stockToggle.setAttribute("aria-pressed", String(stockVisible));
    drawToolpath();
  });

  /**
   * Shows a freshly parsed stock mesh in the scene: forces visibility, updates
   * the "PROGRAM · STOCK" footer label and repaints. Shared by the toolbar
   * file picker below and the file manager's "open 3D file" action.
   * @param {object} mesh - Mesh produced by the STL/OBJ/Three.js parsers.
   * @param {string} fileName - Display name of the stock file.
   */
  function applyStockMesh(mesh, fileName) {
    loadedStockMesh = mesh;
    stockVisible = true;
    stockToggle.classList.add("active");
    stockToggle.setAttribute("aria-pressed", "true");

    const programNameLabel = document.querySelector("#program-name");
    if (programNameLabel) {
      programNameLabel.textContent =
        `${app.getProgram()?.name || t("main.noProgram")} · ${fileName}`;
    }

    drawToolpath();
    document.querySelector("#message").textContent = t("main.stockLoaded", { name: fileName });
  }

  initStockLoader({
    input: document.querySelector("#stock-file"),
    loadButton: document.querySelector("#load-stock"),
    onLoad: (mesh, file) => applyStockMesh(mesh, file.name),
    onError(error) {
      document.querySelector("#message").textContent = t("main.stockLoadError", {
        message: error.message,
      });
    },
  });

  // Keep the handle so the projection toggle tooltip can be re-localized on
  // runtime language switches (updateProjectionToggle writes title/aria via t()).
  const viewerToolbar = initViewerToolbar({ camera, render: drawToolpath });

  /**
   * Mirrors machine position and spindle RPM from a status snapshot into the
   * scene, then syncs the rotation animation and repaints. Called at the tail
   * of every successful status poll.
   * @param {object} status - Status snapshot from the backend.
   */
  function applyStatus(status) {
    toolPosition[0] = Number(status.x) || 0;
    toolPosition[1] = Number(status.y) || 0;
    toolPosition[2] = Number(status.z) || 0;
    toolSpindleRpm = Math.max(0, Number(status.spindle_rpm) || 0);
    toolAnimation.sync();
    drawToolpath();
  }

  /**
   * Stops the spindle rotation animation after a connection error: the tool
   * must not keep spinning while the machine is considered offline.
   */
  function clearTool() {
    toolSpindleRpm = 0;
    toolAnimation.sync();
  }

  return {
    camera,
    toolPosition,
    programName,
    draw: drawToolpath,
    drawTool: sceneViewer.drawTool,
    viewerToolbar,
    applyStatus,
    clearTool,
    // Exposed for the file manager: opened 3D files reuse the exact same
    // pipeline as the toolbar stock picker.
    applyStockMesh,
  };
}
