const INCREMENTAL_DIAL_TITLE =
  "Пошаговый XY Jog: выберите направление на диске, кольцо задаёт расстояние перемещения";
const CONTINUOUS_DIAL_TITLE =
  "Непрерывный XY Jog: удерживайте кнопку мыши на секторе; перемещайте указатель по диску для изменения направления";

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

    const stepsPanel = document.querySelector("#jog-dial-z-steps");
    const continuousPanel = document.querySelector("#jog-dial-z-continuous");
    if (stepsPanel) stepsPanel.hidden = !incremental;
    if (continuousPanel) continuousPanel.hidden = incremental;

    dialCanvas.title = incremental ? INCREMENTAL_DIAL_TITLE : CONTINUOUS_DIAL_TITLE;
    clearHoveredSelection();

    if (dialHint) {
      dialHint.textContent = incremental
        ? `Наведите на сектор: кольца задают шаг ${getStepValues().map(String).join(" / ")} mm`
        : "Удерживайте кнопку и перемещайте указатель по диску";
    }

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
}
