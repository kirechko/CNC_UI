import { dialDirectionAt, dialSelectionAt } from "./jog-dial-hit-test.js";

const DIRECTION_LABELS = [
  "+X",
  "+X / −Y",
  "−Y",
  "−X / −Y",
  "−X",
  "−X / +Y",
  "+Y",
  "+X / +Y",
];

const INCREMENTAL_TITLE =
  "Пошаговый XY Jog: выберите направление на диске, кольцо задаёт расстояние перемещения";
const CONTINUOUS_TITLE =
  "Непрерывный XY Jog: удерживайте кнопку мыши на секторе; перемещайте указатель по диску для изменения направления";

function selectionAt(event, canvas, mode, stepValues) {
  const rect = canvas.getBoundingClientRect();
  const x = event.clientX - rect.left;
  const y = event.clientY - rect.top;

  return mode === "continuous"
    ? dialDirectionAt(x, y, rect.width, rect.height)
    : dialSelectionAt(x, y, rect.width, rect.height, stepValues);
}

export function initJogDialInteraction({
  canvas,
  getMode,
  getConnected,
  getStepValues,
  getHoveredSelection,
  onHover,
  onContinuousSelection,
  onIncrementalSelection,
  stopContinuousJog,
  hint,
}) {
  let capturedPointerId = null;

  function updateHover(event) {
    const mode = getMode();
    const selection = selectionAt(event, canvas, mode, getStepValues());

    if (mode === "continuous" && capturedPointerId === event.pointerId) {
      onContinuousSelection(selection);
    }

    const previous = getHoveredSelection();
    if (
      selection?.sector === previous?.sector &&
      selection?.step === previous?.step
    ) return;

    onHover(selection);
    const direction = DIRECTION_LABELS[selection?.sector];
    canvas.title = selection
      ? mode === "continuous"
        ? `${direction}: удерживайте кнопку мыши для непрерывного перемещения; перемещайте указатель для смены направления`
        : `${direction}: нажмите для перемещения на ${selection.step} mm`
      : mode === "continuous"
        ? "Непрерывный XY Jog: удерживайте кнопку на секторе; центр диска — без движения"
        : `Пошаговый XY Jog: нажмите сектор; кольца задают шаг ${getStepValues().join(" / ")} mm`;

    if (hint) {
      hint.textContent = selection
        ? mode === "continuous"
          ? `${direction} · удерживайте для перемещения`
          : `${direction} · ${selection.step} mm`
        : mode === "continuous"
          ? "Удерживайте кнопку и перемещайте указатель по диску"
          : `Наведите на сектор: кольца задают шаг ${getStepValues().join(" / ")} mm`;
    }
  }

  function clearHover() {
    onHover(null);
    canvas.title = getMode() === "continuous" ? CONTINUOUS_TITLE : INCREMENTAL_TITLE;
    if (hint) {
      hint.textContent = getMode() === "continuous"
        ? "Удерживайте кнопку и перемещайте указатель по диску"
        : `Наведите на сектор: кольца задают шаг ${getStepValues().join(" / ")} mm`;
    }
  }

  function endContinuousJog() {
    capturedPointerId = null;
    if (getMode() === "continuous") stopContinuousJog();
  }

  canvas.addEventListener("pointermove", updateHover);
  canvas.addEventListener("pointerleave", clearHover);

  canvas.addEventListener("pointerdown", (event) => {
    if (getMode() !== "continuous" || !getConnected()) return;

    const selection = selectionAt(event, canvas, "continuous", getStepValues());
    if (!selection) return;

    event.preventDefault();
    capturedPointerId = event.pointerId;
    canvas.setPointerCapture(event.pointerId);
    onContinuousSelection(selection);
  });

  canvas.addEventListener("pointerup", endContinuousJog);
  canvas.addEventListener("pointercancel", endContinuousJog);
  canvas.addEventListener("lostpointercapture", endContinuousJog);

  canvas.addEventListener("click", (event) => {
    if (getMode() === "continuous") return;
    onIncrementalSelection(
      selectionAt(event, canvas, "incremental", getStepValues()),
    );
  });

  canvas.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();

    const selection = getHoveredSelection();
    if (selection) onIncrementalSelection(selection);
  });

  return { clearHover };
}
