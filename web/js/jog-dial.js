// JOG dial feature: the canvas dial (drawing, hover, hit-test selection,
// incremental/continuous dispatch), the dial layout controls (mode buttons,
// Z-fit shrink, dial-fit scale, rotary columns), the speed/step settings
// dialog and the step value propagation into selects and Z buttons.
// Extracted verbatim from main.js; shares `ctx` with jog-motion.js and reads
// machine state through `app`.
import { t } from "./i18n.js";
import { dialMoves } from "./jog-dial-hit-test.js";
import { initJogModeControls } from "./jog-mode.js";
import { initJogZFit } from "./jog-z-fit.js";
import { initJogDialFit } from "./jog-dial-fit.js";
import { initJogRotaryColumns } from "./jog-rotary-columns.js";
import { initJogSpeedDialog, saveJogSpeeds, sendJogSpeeds } from "./jog-speed-dialog.js";
import { initJogDialInteraction } from "./jog-dial-interaction.js";
import { renderJogDial as renderJogDialCanvas } from "./jog-dial-renderer.js";
import { formatJogStep } from "./jog-step-settings.js";

/**
 * Creates the JOG dial controller and wires all dial-related DOM listeners.
 *
 * @param {object} app - Shared application bag exposing `getConnected()` and
 *   `refreshStatus()`.
 * @param {object} ctx - Mutable JOG context from jog-context.js.
 * @param {object} jogMotion - Motion handle from jog-motion.js (provides
 *   `stopContinuousJog` and `bindContinuousButton`).
 * @returns {object} Dial handle: `drawDial`, `updateStepControls`,
 *   `modeControls`, `interaction`, `rotaryColumns`, `isDialVisible`.
 */
export function initJogDial(app, ctx, jogMotion) {
  const jogIncrementalMode = document.querySelector("#jog-incremental-mode");
  const jogContinuousMode = document.querySelector("#jog-continuous-mode");

  const jogKeysMode = document.querySelector("#jog-keys-mode");
  const jogDialMode = document.querySelector("#jog-dial-mode");
  const jogKeysPanel = document.querySelector("#jog-keys-panel");
  const jogDialPanel = document.querySelector("#jog-dial-panel");
  const jogDialCanvas = document.querySelector("#jog-dial");
  const jogDialContext = jogDialCanvas.getContext("2d");
  const jogDialHint = document.querySelector("#jog-dial-hint");
  let hoveredDialSelection = null;

  // The dial canvas paint is called from the mode controls, the step updates,
  // the hover handler, the resize/theme listeners and both fit controllers, so
  // it lives here as a hoisted function over the shared context values.
  function drawJogDial() {
    renderJogDialCanvas({
      canvas: jogDialCanvas,
      context: jogDialContext,
      mode: ctx.motionMode,
      stepValues: ctx.xySteps,
      hoveredSelection: hoveredDialSelection,
    });
  }

  // Handle exposes refreshDialChrome() so the locale listener can re-translate
  // the dial tooltip and hint line immediately after a language switch.
  const jogModeControls = initJogModeControls({
    incrementalButton: jogIncrementalMode,
    continuousButton: jogContinuousMode,
    xyStepSelect: ctx.distanceSelect,
    zStepSelect: ctx.zDistanceSelect,
    keysButton: jogKeysMode,
    dialButton: jogDialMode,
    keysPanel: jogKeysPanel,
    dialPanel: jogDialPanel,
    dialCanvas: jogDialCanvas,
    dialHint: jogDialHint,
    stopContinuousJog: jogMotion.stopContinuousJog,
    drawDial: drawJogDial,
    getStepValues: () => ctx.xySteps,
    clearHoveredSelection: () => {
      hoveredDialSelection = null;
    },
    onMotionModeChange: (mode) => {
      ctx.motionMode = mode;
    },
  });

  // Hide the stepped Z buttons one by one (innermost first) when the JOG panel gets short.
  // The fit observes the panel body, the dial layout, the Z column and the dial canvas.
  initJogZFit({
    steps: document.querySelector("#jog-dial-z-steps"),
    layout: document.querySelector("#jog-dial-panel .jog-dial-layout"),
    boundary: document.querySelector(".jog-panel .panel-body"),
    dialCanvas: jogDialCanvas,
    rotaryColumns: document.querySelector("#jog-rotary-columns"),
  });

  // Shrink the dial itself (down to half of its natural size) when the JOG panel
  // is squeezed so short that the content would otherwise scroll. Must run after
  // the Z fit so both ResizeObservers settle in the same animation frames.
  // onDialResize repaints the canvas from its new CSS box.
  initJogDialFit({
    panel: document.querySelector(".jog-panel"),
    body: document.querySelector(".jog-panel .panel-body"),
    dialPanel: jogDialPanel,
    dialCanvas: jogDialCanvas,
    onDialResize: () => {
      if (!jogDialPanel.hidden) drawJogDial();
    },
  });

  // Rotary jog columns (A, B, C) to the right of the Z column. Each step button has a fixed distance.
  const rotaryColumns = initJogRotaryColumns({
    container: document.querySelector("#jog-rotary-columns"),
  });

  // JOG speed settings: the dialog applies values through POST /api/jog/speed.
  initJogSpeedDialog({
    dialog: document.querySelector("#jog-speed-dialog"),
    form: document.querySelector("#jog-speed-form"),
    openButton: document.querySelector("#jog-speed-open"),
    cancelButton: document.querySelector("#jog-speed-cancel"),
    errorElement: document.querySelector("#jog-speed-error"),
    getSpeeds: () => ctx.speeds,
    getSteps: () => ({ xy: ctx.xySteps, z: ctx.zSteps }),
    async onApply({ speeds, steps }) {
      // Speeds go to the server first; if that fails, nothing else is changed.
      await sendJogSpeeds(speeds);
      ctx.speeds = speeds;
      saveJogSpeeds(speeds);

      // Step groups are applied to the page and kept in local storage.
      ctx.xySteps = steps.xy;
      ctx.zSteps = steps.z;
      for (const group of ["xy", "z"]) {
        try {
          localStorage.setItem(`cnc-ui-jog-steps-${group}`, JSON.stringify(steps[group]));
        } catch {
          // Settings stay active until the page is closed.
        }
      }
      updateJogStepControls();
      ctx.feedback.textContent = t("main.jogSpeedsApplied");
      ctx.feedback.className = "panel-caption";
    },
  });

  // Push the stored speeds to the server once at startup so the controller uses them.
  sendJogSpeeds(ctx.speeds).catch((error) => {
    ctx.feedback.textContent = t("common.errorPrefix", { message: error.message });
    ctx.feedback.className = "error";
  });

  /**
   * Rebuilds the XY/Z step selects, the stepped dial Z buttons and the dial
   * hint line from the current step arrays. Also repaints the dial because the
   * ring labels embed the step values.
   */
  function updateJogStepControls() {
    const updateDistanceSelect = (select, steps) => {
      const previousSelection = Number(select.value);
      select.replaceChildren();

      for (const step of steps) {
        const option = document.createElement("option");
        option.value = formatJogStep(step);
        // The unit word is localized; the numeric value stays technical.
        option.textContent = t("jog.stepOption", { step: formatJogStep(step) });
        select.append(option);
      }

      select.value = steps.includes(previousSelection)
        ? formatJogStep(previousSelection)
        : formatJogStep(steps[Math.min(1, steps.length - 1)]);
    };

    updateDistanceSelect(ctx.distanceSelect, ctx.xySteps);
    updateDistanceSelect(ctx.zDistanceSelect, ctx.zSteps);

    const zButtons = [...document.querySelectorAll('[data-dial-axis="Z"]')];
    for (const direction of [1, -1]) {
      const directionalButtons = zButtons.filter(
        (button) => Number(button.dataset.direction) === direction,
      );
      const steps = direction === 1 ? [...ctx.zSteps].reverse() : ctx.zSteps;

      directionalButtons.forEach((button, index) => {
        const step = steps[index];
        const label = direction < 0 ? `−${formatJogStep(step)}` : formatJogStep(step);
        button.dataset.distance = formatJogStep(step);
        button.textContent = label;
        button.setAttribute(
          "aria-label",
          t("jog.z.ariaLabel", {
            direction: t(direction > 0 ? "jog.direction.plus" : "jog.direction.minus"),
            step: formatJogStep(step),
          }),
        );
        button.title = t("jog.z.title", {
          direction: t(direction > 0 ? "jog.direction.up" : "jog.direction.down"),
          step: formatJogStep(step),
        });
      });
    }

    if (jogDialHint) {
      jogDialHint.textContent = t("jog.dial.hintIncremental", {
        steps: ctx.xySteps.map(formatJogStep).join(" / "),
      });
    }
    drawJogDial();
  }

  updateJogStepControls();

  function setContinuousDialDirection(selection) {
    const moves = dialMoves(selection);
    const xDirection = moves?.find(([axis]) => axis === "X")?.[1] || 0;
    const yDirection = moves?.find(([axis]) => axis === "Y")?.[1] || 0;

    if (
      xDirection === ctx.continuousXy.X &&
      yDirection === ctx.continuousXy.Y
    ) return;

    ctx.continuousXy.X = xDirection;
    ctx.continuousXy.Y = yDirection;
    if (xDirection) ctx.activeAxes.add("X");
    else ctx.activeAxes.delete("X");
    if (yDirection) ctx.activeAxes.add("Y");
    else ctx.activeAxes.delete("Y");

    ctx.api.sendContinuousXY(xDirection, yDirection).catch((error) => {
      ctx.continuousXy.X = 0;
      ctx.continuousXy.Y = 0;
      ctx.activeAxes.delete("X");
      ctx.activeAxes.delete("Y");
      ctx.feedback.textContent = t("main.jogError.continuous", { message: error.message });
      ctx.feedback.className = "error";
    });
  }

  async function jogDialSelection(selection) {
    if (!selection || !app.getConnected()) return;

    const moves = dialMoves(selection);
    if (!moves) return;

    try {
      if (moves.length === 2) {
        await ctx.api.sendDialXyJog(moves[0][1], moves[1][1], selection.step);
      } else {
        await ctx.api.sendDialJog(moves[0][0], moves[0][1], selection.step);
      }
      ctx.feedback.textContent = t("jog.feedback.axes", {
        moves: moves.map(([axis, direction]) => `${axis}${direction > 0 ? "+" : "−"}`).join(" / "),
        step: selection.step,
      });
      ctx.feedback.className = "panel-caption";
      await app.refreshStatus();
    } catch (error) {
      ctx.feedback.textContent = t("common.errorPrefix", { message: error.message });
      ctx.feedback.className = "error";
    }
  }

  // Handle exposes clearHover() so the locale listener can drop the stale
  // English hover tooltip/hint from the dial right after a language switch.
  const jogDialInteraction = initJogDialInteraction({
    canvas: jogDialCanvas,
    getMode: () => ctx.motionMode,
    getConnected: () => app.getConnected(),
    getStepValues: () => ctx.xySteps,
    getHoveredSelection: () => hoveredDialSelection,
    onHover(selection) {
      hoveredDialSelection = selection;
      drawJogDial();
    },
    onContinuousSelection: setContinuousDialDirection,
    onIncrementalSelection: jogDialSelection,
    stopContinuousJog: jogMotion.stopContinuousJog,
    hint: jogDialHint,
  });

  // Incremental stepped Z buttons rendered as HTML next to the dial.
  document.querySelectorAll("[data-dial-axis]").forEach((button) => {
    button.addEventListener("click", async () => {
      if (ctx.motionMode === "continuous") return;
      try {
        await ctx.api.sendDialJog(
          button.dataset.dialAxis,
          Number(button.dataset.direction),
          Number(button.dataset.distance),
        );
        ctx.feedback.textContent = t("jog.feedback.z", {
          sign: button.dataset.direction === "1" ? "+" : "−",
          distance: button.dataset.distance,
        });
        ctx.feedback.className = "panel-caption";
        await app.refreshStatus();
      } catch (error) {
        ctx.feedback.textContent = t("common.errorPrefix", { message: error.message });
        ctx.feedback.className = "error";
      }
    });
  });

  return {
    drawDial: drawJogDial,
    updateStepControls: updateJogStepControls,
    modeControls: jogModeControls,
    interaction: jogDialInteraction,
    rotaryColumns,
    isDialVisible: () => !jogDialPanel.hidden,
  };
}
