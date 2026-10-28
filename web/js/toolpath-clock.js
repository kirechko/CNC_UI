// Toolpath remaining-time clock: estimates how long the loaded program still
// takes to run from backend traversed-distance telemetry, smooths the estimate
// between polls, and renders "remaining/total" into the viewer footer.
// Extracted verbatim from main.js; program and status come from the shared
// `app` bag so the clock always reads the latest values lazily.
import {
  estimateToolpathTime,
  formatDuration,
  formatRemainingDuration,
} from "./toolpath-time.js";

/**
 * Creates the toolpath clock.
 *
 * @param {object} app - Shared application bag exposing `getStatus()` and
 *   `getProgram()`.
 * @returns {{ render: () => void, update: (status?: object) => void }}
 *   `render` repaints the label from the current anchor (interval-driven);
 *   `update` re-anchors the clock from a fresh status snapshot.
 */
export function initToolpathClock(app) {
  let toolpathRemainingAnchor = null;
  let toolpathTimeAnchorAt = 0;
  let toolpathTotalSeconds = 0;
  let toolpathClockRunning = false;

  function renderToolpathTime() {
    const element = document.querySelector("#toolpath-time");
    if (!element || toolpathRemainingAnchor === null) return;

    const elapsed = toolpathClockRunning
      ? Math.max(0, (performance.now() - toolpathTimeAnchorAt) / 1000)
      : 0;
    const remaining = Math.max(0, toolpathRemainingAnchor - elapsed);
    element.textContent =
      `${formatRemainingDuration(remaining)}/${formatDuration(toolpathTotalSeconds)}`;
    element.hidden = false;
  }

  /**
   * Re-estimates the remaining time from backend progress. Called on every
   * status poll; the smoothing keeps arcs and line changes steady.
   * @param {object} [status=app.getStatus()] - Latest status snapshot.
   */
  function updateToolpathTime(status = app.getStatus()) {
    const loadedProgram = app.getProgram();
    const element = document.querySelector("#toolpath-time");
    if (!element) return;

    if (!loadedProgram?.segments?.length) {
      toolpathRemainingAnchor = null;
      toolpathClockRunning = false;
      element.hidden = true;
      return;
    }

    // Traversed distance comes from the backend progress; the browser never infers it.
    const estimate = estimateToolpathTime(
      loadedProgram.segments,
      status?.configuration,
      Number.isFinite(status?.progress?.path_distance_mm)
        ? status.progress.path_distance_mm
        : null,
    );
    const now = performance.now();
    const isRunning =
      status?.state === "READING" || status?.state === "RUNNING";
    let remaining = estimate.remainingSeconds;

    if (
      isRunning &&
      toolpathClockRunning &&
      toolpathRemainingAnchor !== null
    ) {
      const elapsed = Math.max(0, (now - toolpathTimeAnchorAt) / 1000);
      const predicted = Math.max(0, toolpathRemainingAnchor - elapsed);
      // Smooth the position estimate over time rather than by the size of
      // the telemetry jump. This reduces jitter on arcs and at line changes.
      const smoothingTimeSeconds = 4;
      const alpha = 1 - Math.exp(-elapsed / smoothingTimeSeconds);
      remaining = Math.max(
        0,
        predicted + (estimate.remainingSeconds - predicted) * alpha,
      );
    }

    toolpathRemainingAnchor = remaining;
    toolpathTimeAnchorAt = now;
    toolpathTotalSeconds = estimate.totalSeconds;
    toolpathClockRunning = isRunning;
    renderToolpathTime();
  }

  return { render: renderToolpathTime, update: updateToolpathTime };
}
