import { dialDirectionAt, dialSelectionAt } from "./jog-dial-hit-test.js";
import { t } from "./i18n.js";

// Dial sector labels are axis symbols (+X, −Y, ...) shared by every locale, so
// they stay untranslated; only the surrounding sentences are localized below.
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

/** Idle (not hovered) canvas tooltip for the incremental dial mode. */
function incrementalTitle() {
  return t("jog.dial.incrementalTitle");
}

/** Idle (not hovered) canvas tooltip for the continuous dial mode. */
function continuousTitle() {
  return t("jog.dial.continuousTitle");
}

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
    // The sector name is a technical axis symbol; it is injected into the
    // localized template through the {direction} placeholder.
    const direction = DIRECTION_LABELS[selection?.sector];
    canvas.title = selection
      ? mode === "continuous"
        ? t("jog.dial.hoverContinuous", { direction })
        : t("jog.dial.hoverIncremental", { direction, step: selection.step })
      : mode === "continuous"
        ? t("jog.dial.centerContinuous")
        : t("jog.dial.centerIncremental", { steps: getStepValues().join(" / ") });

    if (hint) {
      hint.textContent = selection
        ? mode === "continuous"
          ? t("jog.dial.hintHoverContinuous", { direction })
          : t("jog.dial.hintHoverIncremental", { direction, step: selection.step })
        : mode === "continuous"
          ? t("jog.dial.hintContinuous")
          : t("jog.dial.hintIncremental", { steps: getStepValues().join(" / ") });
    }
  }

  /**
   * Removes the hovered selection and restores the idle tooltip/hint. Also
   * used by main.js on the cnc-locale-changed event so stale English strings
   * disappear immediately after a language switch.
   */
  function clearHover() {
    onHover(null);
    canvas.title = getMode() === "continuous" ? continuousTitle() : incrementalTitle();
    if (hint) {
      hint.textContent = getMode() === "continuous"
        ? t("jog.dial.hintContinuous")
        : t("jog.dial.hintIncremental", { steps: getStepValues().join(" / ") });
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
