/*
 * JOG settings dialog: step distances for XY and Z, and feed speeds for XY, Z and rotary (A–C).
 *
 * The dialog holds two groups of values:
 * - Steps (XY and Z): four increasing millimetre values used by the dial rings and the
 *   Z step buttons. They are kept in local storage and applied to the page only.
 * - Speeds (XY, Z and rotary A–C): kept in local storage and sent to POST /api/jog/speed,
 *   where the controller uses them for every following jog command.
 */

import { formatJogStep, isValidJogStepValues } from "./jog-step-settings.js";
import { t } from "./i18n.js";

/** Default speeds; must match the server defaults in cnc_controller.rs. */
export const JOG_SPEED_DEFAULTS = Object.freeze({ xy: 1000, z: 300, rotary: 300 });

/** Local storage key that keeps the speeds between page loads. */
const STORAGE_KEY = "cnc-ui-jog-speeds";

/** Upper limit accepted for a speed value; matches the server validation. */
const MAX_SPEED = 100000;

/** Keys of the speed settings in the order they appear in the dialog. */
const SPEED_KEYS = ["xy", "z", "rotary"];

/** Human-readable names of the speed settings used in error messages. */
const SPEED_LABELS = { xy: "XY", z: "Z", rotary: "A–C" };

/** Step groups in the order they appear in the dialog. */
const STEP_GROUPS = ["xy", "z"];

/**
 * Checks that a speed value is a finite positive number within the allowed range.
 *
 * @param {number} value - Candidate speed.
 * @returns {boolean} True when the value can be used as a JOG speed.
 */
export function isValidJogSpeed(value) {
  return Number.isFinite(value) && value > 0 && value <= MAX_SPEED;
}

/**
 * Loads the stored speeds, falling back to the defaults for missing or invalid data.
 *
 * @returns {{xy: number, z: number, rotary: number}} Speed settings.
 */
export function loadJogSpeeds() {
  try {
    const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null");
    if (stored && SPEED_KEYS.every((key) => isValidJogSpeed(Number(stored[key])))) {
      return {
        xy: Number(stored.xy),
        z: Number(stored.z),
        rotary: Number(stored.rotary),
      };
    }
  } catch {
    // Broken storage content: use the defaults below.
  }
  return { ...JOG_SPEED_DEFAULTS };
}

/**
 * Saves the speeds to local storage. Failures are ignored; the values stay active until reload.
 *
 * @param {{xy: number, z: number, rotary: number}} values - Speed settings to store.
 */
export function saveJogSpeeds(values) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(values));
  } catch {
    // Storage may be unavailable (private mode, quota); the speeds still apply to the server.
  }
}

/**
 * Sends the speed settings to the server.
 *
 * @param {{xy?: number, z?: number, rotary?: number}} values - Speeds to apply.
 * @returns {Promise<void>} Resolves when the server accepted the values.
 * @throws {Error} When the request fails or the server rejects the values.
 */
export async function sendJogSpeeds(values) {
  const response = await fetch("/api/jog/speed", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(values),
  });

  if (!response.ok) {
    const detail = await response.text();
    throw new Error(detail || `HTTP ${response.status}`);
  }
}

/**
 * Wires the JOG settings dialog: opening with current values, validation and apply.
 *
 * @param {Object} options - Dialog elements and callbacks.
 * @param {HTMLDialogElement} options.dialog - The JOG settings dialog.
 * @param {HTMLFormElement} options.form - Form with speed inputs (xy, z, rotary) and step inputs (xyStep, zStep).
 * @param {HTMLButtonElement} options.openButton - Button that opens the dialog.
 * @param {HTMLButtonElement} options.cancelButton - Button that closes the dialog without applying.
 * @param {HTMLElement} options.errorElement - Element that shows validation or send errors.
 * @param {Function} options.getSpeeds - Returns the currently active speeds.
 * @param {Function} options.getSteps - Returns the currently active steps as {xy: number[], z: number[]}.
 * @param {Function} options.onApply - Called with {speeds, steps} after validation; may return a promise.
 */
export function initJogSpeedDialog({
  dialog,
  form,
  openButton,
  cancelButton,
  errorElement,
  getSpeeds,
  getSteps,
  onApply,
}) {
  // Map of speed key to its input element inside the form.
  const speedInputs = Object.fromEntries(SPEED_KEYS.map((key) => [key, form.elements[key]]));
  // Map of step group to its four input elements inside the form.
  const stepInputs = Object.fromEntries(
    STEP_GROUPS.map((group) => [
      group,
      [...form.querySelectorAll(`input[name="${group}Step"]`)],
    ]),
  );

  /**
   * Shows an error message in the dialog.
   *
   * @param {string} message - Text to show.
   */
  function showError(message) {
    errorElement.textContent = message;
    errorElement.hidden = false;
  }

  // Fill every input with the current values before showing the dialog.
  openButton.addEventListener("click", () => {
    const speeds = getSpeeds();
    for (const key of SPEED_KEYS) {
      speedInputs[key].value = String(speeds[key]);
    }

    const steps = getSteps();
    for (const group of STEP_GROUPS) {
      stepInputs[group].forEach((input, index) => {
        input.value = formatJogStep(steps[group][index]);
      });
    }

    errorElement.hidden = true;
    dialog.showModal();
  });

  cancelButton.addEventListener("click", () => {
    dialog.close();
  });

  form.addEventListener("submit", async (event) => {
    event.preventDefault();

    // Validate speeds first; each must be a positive number within the server limit.
    const speeds = {};
    for (const key of SPEED_KEYS) {
      const value = Number(speedInputs[key].value);
      if (!isValidJogSpeed(value)) {
        // SPEED_LABELS stay technical (XY, Z, A–C); only the sentence is localized.
        showError(t("jog.speedDialog.errorSpeed", { label: SPEED_LABELS[key], max: MAX_SPEED }));
        return;
      }
      speeds[key] = value;
    }

    // Validate each group of four steps: increasing, above 0, up to 100000.
    const steps = {};
    for (const group of STEP_GROUPS) {
      const values = stepInputs[group].map((input) => Number(input.value));
      if (!isValidJogStepValues(values)) {
        // The step group name (XY/Z) is a technical axis symbol; the sentence is localized.
        showError(t("jog.speedDialog.errorSteps", { group: group.toUpperCase(), max: MAX_SPEED }));
        return;
      }
      steps[group] = values;
    }

    try {
      await onApply({ speeds, steps });
      dialog.close();
    } catch (error) {
      showError(error.message);
    }
  });
}
