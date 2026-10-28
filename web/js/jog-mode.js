import { t } from "./i18n.js";

export function initJogModeControls({
  incrementalButton,
  continuousButton,
  xyStepSelect,
  zStepSelect,
  keysButton,
  dialButton,
  keysPanel,
  dialPanel,
  dialCanvas,
  dialHint,
  stopContinuousJog,
  drawDial,
  getStepValues,
  clearHoveredSelection,
  onMotionModeChange,
}) {
  /**
   * Applies the localized idle tooltip and hint text of the dial for the given
   * motion mode. Used on every mode switch and from main.js on locale change.
   *
   * @param {boolean} incremental - True for incremental mode, false for continuous.
   */
  function refreshDialChrome(incremental) {
    dialCanvas.title = t(incremental ? "jog.dial.incrementalTitle" : "jog.dial.continuousTitle");

    if (dialHint) {
      dialHint.textContent = incremental
        ? t("jog.dial.hintIncremental", { steps: getStepValues().map(String).join(" / ") })
        : t("jog.dial.hintContinuous");
    }
  }

  function setMotionMode(mode) {
    const incremental = mode === "incremental";
    onMotionModeChange?.(mode);

    incrementalButton.classList.toggle("active", incremental);
    continuousButton.classList.toggle("active", !incremental);
    incrementalButton.setAttribute("aria-pressed", String(incremental));
    continuousButton.setAttribute("aria-pressed", String(!incremental));
    xyStepSelect.disabled = !incremental;
    zStepSelect.disabled = !incremental;

    if (incremental) stopContinuousJog();

    // Z and rotary columns (A–C) share the same incremental/continuous switch.
    const stepsPanels = document.querySelectorAll("#jog-dial-z-steps, .jog-rotary-steps");
    const continuousPanels = document.querySelectorAll("#jog-dial-z-continuous, .jog-rotary-continuous");
    stepsPanels.forEach((panel) => { panel.hidden = !incremental; });
    continuousPanels.forEach((panel) => { panel.hidden = incremental; });

    refreshDialChrome(incremental);
    clearHoveredSelection();

    if (!dialPanel.hidden) drawDial();
  }

  function setViewMode(mode) {
    const showDial = mode === "dial";
    keysPanel.hidden = showDial;
    dialPanel.hidden = !showDial;
    keysButton.classList.toggle("active", !showDial);
    dialButton.classList.toggle("active", showDial);
    keysButton.setAttribute("aria-pressed", String(!showDial));
    dialButton.setAttribute("aria-pressed", String(showDial));

    if (showDial) drawDial();
  }

  incrementalButton.addEventListener("click", () => setMotionMode("incremental"));
  continuousButton.addEventListener("click", () => setMotionMode("continuous"));
  keysButton.addEventListener("click", () => setViewMode("keys"));
  dialButton.addEventListener("click", () => setViewMode("dial"));

  setMotionMode("incremental");
  setViewMode("dial");

  // Expose the chrome refresher so main.js can re-localize the dial tooltip
  // and hint immediately after a runtime language switch.
  return { refreshDialChrome };
}
