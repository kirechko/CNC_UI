// Shared JOG context: the mutable state and DOM references that every JOG
// feature (motion buttons, dial, mode controls, keyboard) reads and writes.
// Created once by main.js and injected into the other jog modules, so there
// is a single source of truth for step values, speeds, motion mode and the
// in-flight continuous-jog bookkeeping. Extracted verbatim from main.js.
import { initJogApi } from "./jog-api.js";
import { loadJogStepValues } from "./jog-step-settings.js";
import { loadJogSpeeds } from "./jog-speed-dialog.js";

/**
 * Builds the JOG context object.
 *
 * @param {object} app - Shared application bag exposing `getConnected()` for
 *   the jog API guard.
 * @returns {object} Mutable context with refs, steps, speeds, motion mode,
 *   continuous-jog state and the jog API handle.
 */
export function createJogContext(app) {
  const feedback = document.querySelector("#message");
  const jogButtons = [
    ...document.querySelectorAll("[data-axis], [data-x-direction][data-y-direction]"),
  ];
  const distanceSelect = document.querySelector("#distance");
  const zDistanceSelect = document.querySelector("#distance-z");
  // Per-axis step selects of the keypad rotary columns (task: A–C step lists
  // in the button JOG mode). Missing elements are tolerated: the rotary axis
  // then falls back to the XY select, exactly as before this feature.
  const rotaryDistanceSelects = new Map(
    ["A", "B", "C"].map((axis) => [axis, document.querySelector(`#distance-${axis.toLowerCase()}`)]),
  );
  const positionDialog = document.querySelector("#position-dialog");
  const jogSpeedDialog = document.querySelector("#jog-speed-dialog");

  // Load the jog step presets: the per-axis keys fall back to the legacy
  // combined key so upgrades keep the user's previous configuration.
  const legacyJogStepValues = loadJogStepValues(
    "cnc-ui-jog-steps",
    [0.1, 1, 10, 100],
  );

  const ctx = {
    feedback,
    jogButtons,
    distanceSelect,
    zDistanceSelect,
    rotaryDistanceSelects,
    positionDialog,
    jogSpeedDialog,
    xySteps: loadJogStepValues("cnc-ui-jog-steps-xy", legacyJogStepValues),
    zSteps: loadJogStepValues("cnc-ui-jog-steps-z", legacyJogStepValues),
    // Feed speeds for XY, Z and rotary (A–C) JOG; loaded from local storage, defaults otherwise.
    speeds: loadJogSpeeds(),
    motionMode: "incremental",
    // Current continuous XY direction vector (diagonal jog keeps both live).
    continuousXy: { X: 0, Y: 0 },
    // Axes with a live continuous jog; drives stop/start deduplication.
    activeAxes: new Set(),
    api: initJogApi({ isConnected: () => app.getConnected() }),
  };

  return ctx;
}
