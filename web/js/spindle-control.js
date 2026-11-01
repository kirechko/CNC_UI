import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/**
 * Initializes the shared spindle/laser panel and keeps its controls aligned with
 * the active controller's output mode and native S-value range.
 */
export function initSpindleControls({ refreshStatus }) {
  const speedSlider = document.querySelector("#spindle-speed-slider");
  const clockwiseButton = document.querySelector("#spindle-cw");
  const counterClockwiseButton = document.querySelector("#spindle-ccw");
  const stepButton = document.querySelector("#spindle-step");
  const laserModeControl = document.querySelector("#laser-mode-control");
  const laserModeSelect = document.querySelector("#laser-mode");
  const valueLabel = document.querySelector("#spindle-value-label");
  const speedOptions = document.querySelector("#spindle-speed-options");
  const presetsContainer = document.querySelector("#spindle-speed-presets");
  const manualSpeedInput = document.querySelector("#spindle-manual-speed");
  const manualSpeedApply = document.querySelector("#spindle-manual-speed-apply");

  let direction = "stopped";
  let pendingDirection = null;
  let commandError = null;
  let spindleRunning = false;
  let minimumRpm = 0;
  let maximumRpm = 0;
  let laserMode = false;
  let laserPowerMaximum = 0;
  let selectedLaserMode = "constant";

  function syncSpeed(value) {
    const rpm = Math.max(
      minimumRpm,
      Math.min(maximumRpm, Math.round(Number(value) || minimumRpm)),
    );
    speedSlider.value = String(rpm);
    stepButton.textContent = String(rpm);
  }

  speedSlider.addEventListener("input", () => syncSpeed(speedSlider.value));

  function speedPresets() {
    const rounding = maximumRpm <= 9000 ? 100 : 1000;
    const values = [minimumRpm];

    for (let index = 1; index <= 4; index++) {
      const evenlySpaced = minimumRpm + (maximumRpm - minimumRpm) * index / 5;
      values.push(Math.round(evenlySpaced / rounding) * rounding);
    }

    values.push(maximumRpm);
    return [...new Set(values.map((value) =>
      Math.max(minimumRpm, Math.min(maximumRpm, value)),
    ))].sort((a, b) => a - b);
  }

  function closeSpeedOptions() {
    speedOptions.hidden = true;
    stepButton.setAttribute("aria-expanded", "false");
  }

  function openSpeedOptions() {
    presetsContainer.replaceChildren();
    manualSpeedInput.min = String(minimumRpm);
    manualSpeedInput.max = String(maximumRpm);
    manualSpeedInput.value = speedSlider.value;
    manualSpeedInput.setCustomValidity("");

    for (const rpm of speedPresets()) {
      const option = document.createElement("button");
      option.className = "spindle-speed-option";
      option.type = "button";
      option.role = "option";
      option.textContent = rpm.toFixed(0);
      // Preset buttons are recreated on every open, so the tooltip always
      // uses the dictionary of the currently active locale.
      option.title = t("html.spindle.presetOptionTitle", { rpm: rpm.toFixed(0) });
      option.setAttribute("aria-selected", String(rpm === Number(speedSlider.value)));
      option.addEventListener("click", () => {
        syncSpeed(rpm);
        closeSpeedOptions();
      });
      presetsContainer.append(option);
    }

    speedOptions.hidden = false;
    stepButton.setAttribute("aria-expanded", "true");
  }

  stepButton.addEventListener("click", () => {
    if (speedOptions.hidden) openSpeedOptions();
    else closeSpeedOptions();
  });

  function applyManualSpeed() {
    const rawValue = manualSpeedInput.value.trim();
    const rpm = Number(rawValue);

    if (
      !rawValue ||
      !Number.isFinite(rpm) ||
      rpm < minimumRpm ||
      rpm > maximumRpm
    ) {
      manualSpeedInput.setCustomValidity(
        t("spindle.speedRange", {
          min: minimumRpm.toFixed(0),
          max: maximumRpm.toFixed(0),
        }),
      );
      manualSpeedInput.reportValidity();
      return;
    }

    manualSpeedInput.setCustomValidity("");
    syncSpeed(rpm);
    closeSpeedOptions();
  }

  manualSpeedApply.addEventListener("click", applyManualSpeed);
  manualSpeedInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      applyManualSpeed();
    }
  });
  manualSpeedInput.addEventListener("input", () => {
    manualSpeedInput.setCustomValidity("");
  });

  document.addEventListener("pointerdown", (event) => {
    if (!event.target.closest(".spindle-speed-picker")) closeSpeedOptions();
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && !speedOptions.hidden) {
      closeSpeedOptions();
      stepButton.focus();
    }
  });

  laserModeSelect.addEventListener("change", async () => {
    const previousMode = selectedLaserMode;
    selectedLaserMode = laserModeSelect.value;
    try {
      await invokeOrFetch("set_laser_mode", { mode: selectedLaserMode }, "/api/laser/mode", "POST");
      commandError = null;
      await refreshStatus();
    } catch (error) {
      selectedLaserMode = previousMode;
      laserModeSelect.value = previousMode;
      commandError = t("spindle.error", { message: error.message });
      await refreshStatus();
    }
  });

  async function commandSpindle(requestedDirection) {
    if (
      laserMode &&
      direction !== "cw" &&
      !window.confirm(t("spindle.laserSafetyConfirm"))
    ) return;
    // In laser mode one button is used for both enable and M5 stop; direction has no meaning.
    const requested = laserMode ? "cw" : requestedDirection.toLowerCase();
    const nextDirection = direction === requested ? "stopped" : requested;
    const enabled = nextDirection !== "stopped";
    const rpm = Number(speedSlider.value);

    pendingDirection = nextDirection;
    direction = nextDirection;
    updateButtons();

    try {
      await invokeOrFetch(
        "set_spindle",
        {
          rpm,
          power: rpm,
          enabled,
          direction: laserMode ? "CW" : requestedDirection,
          mode: laserMode ? selectedLaserMode : undefined,
        },
        "/api/spindle",
        "POST",
      );
      commandError = null;
      await refreshStatus();
    } catch (error) {
      pendingDirection = null;
      commandError = t("spindle.error", { message: error.message });
      await refreshStatus();
    }
  }

  clockwiseButton.addEventListener("click", () => commandSpindle("CW"));
  counterClockwiseButton.addEventListener("click", () => commandSpindle("CCW"));

  function updateButtons() {
    const clockwiseActive = direction === "cw";
    const counterClockwiseActive = direction === "ccw";
    const stopIcon = '<rect x="5" y="5" width="14" height="14" rx="2" fill="currentColor" stroke="none"></rect>';
    const clockwiseIcon = '<path d="M20 11a8 8 0 0 0-14.8-4L4 9"></path><path d="M4 4v5h5"></path>';
    const counterClockwiseIcon = '<path d="M4 11a8 8 0 0 1 14.8-4L20 9"></path><path d="M20 4v5h-5"></path>';

    const clockwiseIconState = clockwiseActive ? "stop" : "cw";
    const counterClockwiseIconState = counterClockwiseActive ? "stop" : "ccw";

    clockwiseButton.classList.toggle("active-cw", clockwiseActive);
    counterClockwiseButton.classList.toggle("active-ccw", counterClockwiseActive);
    // Do not replace the SVG on every telemetry update: pointerdown/click must
    // stay on the same DOM element, otherwise the browser may cancel the click.
    if (clockwiseButton.dataset.iconState !== clockwiseIconState) {
      clockwiseButton.innerHTML =
        `<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${clockwiseActive ? stopIcon : clockwiseIcon}</svg>`;
      clockwiseButton.dataset.iconState = clockwiseIconState;
    }
    if (counterClockwiseButton.dataset.iconState !== counterClockwiseIconState) {
      counterClockwiseButton.innerHTML =
        `<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${counterClockwiseActive ? stopIcon : counterClockwiseIcon}</svg>`;
      counterClockwiseButton.dataset.iconState = counterClockwiseIconState;
    }

    // Labels are resolved from the dictionary; the same text is used for
    // the accessible name and the tooltip.
    const clockwiseLabel = clockwiseActive
      ? t("spindle.stop")
      : t("spindle.startClockwise");
    const counterClockwiseLabel = counterClockwiseActive
      ? t("spindle.stop")
      : t("spindle.startCounterClockwise");
    clockwiseButton.setAttribute("aria-label", clockwiseLabel);
    counterClockwiseButton.setAttribute("aria-label", counterClockwiseLabel);
    clockwiseButton.title = clockwiseLabel;
    counterClockwiseButton.title = counterClockwiseLabel;
  }

  /** Updates range, terminology, mode selector, and laser safety affordances from status. */
  function update(status) {
    spindleRunning = Boolean(status.spindle_enabled);
    // This flat capability flag lets the panel degrade to ordinary RPM controls per device.
    laserMode = Boolean(status.laser_mode_available);
    const reportedMode = String(status.spindle_mode || "standard");
    selectedLaserMode = reportedMode === "laser_dynamic" ? "dynamic" : "constant";
    laserPowerMaximum = Math.max(0, Number(status.spindle_max_rpm) || 0);
    minimumRpm = Math.max(0, Number(status.spindle_min_rpm) || 0);
    maximumRpm = Math.max(0, Number(status.spindle_max_rpm) || 0);
    laserModeControl.hidden = !laserMode;
    laserModeSelect.value = selectedLaserMode;
    // Use power terminology only when this machine's configuration advertises a laser.
    valueLabel.textContent = laserMode ? t("html.spindle.power") : t("html.spindle.rpm");
    manualSpeedInput.setAttribute(
      "aria-label",
      laserMode ? t("html.spindle.power") : t("html.spindle.manualAria"),
    );
    speedSlider.dataset.i18nTitle = laserMode
      ? "html.spindle.powerSliderTitle"
      : "html.spindle.sliderTitle";
    speedSlider.max = String(laserMode ? laserPowerMaximum : maximumRpm);
    manualSpeedInput.max = speedSlider.max;
    laserModeSelect.disabled = spindleRunning;
    clockwiseButton.hidden = laserMode;
    counterClockwiseButton.hidden = laserMode;
    document.querySelector("#spindle-control").classList.toggle("laser-active", laserMode && spindleRunning);
    document.querySelector("#spindle-control").classList.toggle("laser-mode", laserMode);

    speedSlider.min = String(minimumRpm);
    speedSlider.max = String(laserMode ? laserPowerMaximum : maximumRpm);
    speedSlider.step = "any";

    const reportedSetpoint = laserMode && status.laser_power != null
      ? Number(status.laser_power)
      : Number(status.spindle_commanded_rpm);
    const currentSetpoint = Number(speedSlider.value);
    const nextSetpoint =
      spindleRunning && Number.isFinite(reportedSetpoint) && reportedSetpoint > 0
        ? reportedSetpoint
        : Number.isFinite(currentSetpoint) && currentSetpoint >= minimumRpm
          ? Math.min(currentSetpoint, maximumRpm)
          : minimumRpm;
    speedSlider.value = String(nextSetpoint);
    stepButton.textContent = String(nextSetpoint);

    const reportedDirection = status.spindle_direction || "stopped";
    const actualRpm = Number(status.spindle_rpm) || 0;

    if (pendingDirection === "stopped" && (!spindleRunning || actualRpm <= 0.01)) {
      pendingDirection = null;
    } else if (
      pendingDirection &&
      pendingDirection !== "stopped" &&
      reportedDirection === pendingDirection &&
      spindleRunning
    ) {
      pendingDirection = null;
    }

    direction = pendingDirection || (spindleRunning ? reportedDirection : "stopped");
    updateButtons();
    speedSlider.disabled =
      !status.connected ||
      maximumRpm <= 0 ||
      spindleRunning ||
      direction !== "stopped";
  }

  return {
    update,
    get commandError() {
      return commandError;
    },
  };
}
