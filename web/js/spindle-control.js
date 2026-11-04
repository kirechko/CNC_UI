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
  const laserToggle = document.querySelector("#laser-toggle");
  const stepButton = document.querySelector("#spindle-step");
  const laserModeIndicator = document.querySelector("#laser-mode-indicator");
  const valueLabel = document.querySelector("#spindle-value-label");
  const speedOptions = document.querySelector("#spindle-speed-options");
  const presetsContainer = document.querySelector("#spindle-speed-presets");
  const manualSpeedInput = document.querySelector("#spindle-manual-speed");
  const manualSpeedApply = document.querySelector("#spindle-manual-speed-apply");
  const panel = document.querySelector("#spindle-control");
  const machineData = { profile: null, controllerLaser: false };

  let direction = "stopped";
  let pendingDirection = null;
  let commandError = null;
  let spindleRunning = false;
  let minimumRpm = 0;
  let maximumRpm = 0;
  let laserMode = false;
  let laserPowerMaximum = 0;
  let minimumPower = 0;
  let currentPowerUnit = "";
  let selectedLaserMode = "constant";

  function syncSpeed(value) {
    const outputMinimum = laserMode ? minimumPower : minimumRpm;
    const outputMaximum = laserMode ? laserPowerMaximum : maximumRpm;
    const rpm = Math.max(
      outputMinimum,
      Math.min(outputMaximum, Math.round(Number(value) || outputMinimum)),
    );
    speedSlider.value = String(rpm);
    stepButton.textContent = String(rpm);
  }

  speedSlider.addEventListener("input", () => syncSpeed(speedSlider.value));

  function speedPresets() {
    const outputMinimum = laserMode ? minimumPower : minimumRpm;
    const outputMaximum = laserMode ? laserPowerMaximum : maximumRpm;
    const rounding = laserMode ? 1 : outputMaximum <= 9000 ? 100 : 1000;
    const values = [outputMinimum];

    for (let index = 1; index <= 4; index++) {
      const evenlySpaced = outputMinimum + (outputMaximum - outputMinimum) * index / 5;
      values.push(Math.round(evenlySpaced / rounding) * rounding);
    }

    values.push(outputMaximum);
    return [...new Set(values.map((value) =>
      Math.max(outputMinimum, Math.min(outputMaximum, value)),
    ))].sort((a, b) => a - b);
  }

  function closeSpeedOptions() {
    speedOptions.hidden = true;
    stepButton.setAttribute("aria-expanded", "false");
  }

  function openSpeedOptions() {
    presetsContainer.replaceChildren();
    const outputMinimum = laserMode ? minimumPower : minimumRpm;
    const outputMaximum = laserMode ? laserPowerMaximum : maximumRpm;
    manualSpeedInput.min = String(outputMinimum);
    manualSpeedInput.max = String(outputMaximum);
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
    const outputMinimum = laserMode ? minimumPower : minimumRpm;
    const outputMaximum = laserMode ? laserPowerMaximum : maximumRpm;

    if (
      !rawValue ||
      !Number.isFinite(rpm) ||
      rpm < outputMinimum ||
      rpm > outputMaximum
    ) {
      manualSpeedInput.setCustomValidity(
        t("spindle.speedRange", {
          min: (laserMode ? minimumPower : minimumRpm).toFixed(0),
          max: (laserMode ? laserPowerMaximum : maximumRpm).toFixed(0),
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

  async function commandSpindle(requestedDirection) {
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
          profileId: machineData.profile?.id,
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
  laserToggle?.addEventListener("click", () => commandSpindle("CW"));

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
    laserToggle?.classList.toggle("running", laserMode && clockwiseActive);
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
    const clockwiseLabel = laserMode ? t(clockwiseActive ? "spindle.laserStop" : "spindle.laserToggle") : (clockwiseActive ? t("spindle.stop") : t("spindle.startClockwise"));
    const counterClockwiseLabel = counterClockwiseActive
      ? t("spindle.stop")
      : t("spindle.startCounterClockwise");
    clockwiseButton.setAttribute("aria-label", clockwiseLabel);
    counterClockwiseButton.setAttribute("aria-label", counterClockwiseLabel);
    clockwiseButton.title = clockwiseLabel;
    if (laserToggle) {
      const laserLabel = laserMode ? t(clockwiseActive ? "spindle.laserStop" : "spindle.laserToggle") : clockwiseLabel;
      laserToggle.setAttribute("aria-label", laserLabel);
      laserToggle.title = laserLabel;
    }
    counterClockwiseButton.title = counterClockwiseLabel;
  }

  /** Updates range, terminology, mode selector, and laser safety affordances from status. */
  function update(status) {
    spindleRunning = Boolean(status.spindle_enabled);
    // This flat capability flag lets the panel degrade to ordinary RPM controls per device.
    machineData.controllerLaser = Boolean(status.laser_mode_available);
    const profileType = machineData.profile?.machineType;
    laserMode = profileType === "laser" || (profileType !== "mill" && machineData.controllerLaser);
    const reportedMode = String(status.spindle_mode || "standard");
    currentPowerUnit = status.configuration?.spindle?.laser?.max_power != null ? "S" : "";
    selectedLaserMode = reportedMode === "laser_dynamic" ? "dynamic" : "constant";
    laserPowerMaximum = Math.max(0, Number(status.configuration?.spindle?.laser?.max_power ?? status.spindle_max_rpm) || 0);
    minimumPower = Math.max(0, Number(status.configuration?.spindle?.laser?.min_power ?? 0) || 0);
    minimumRpm = Math.max(0, Number(status.spindle_min_rpm) || 0);
    maximumRpm = Math.max(0, Number(status.spindle_max_rpm) || 0);
    if (laserModeIndicator) {
      const dynamic = reportedMode === "laser_dynamic";
      laserModeIndicator.hidden = !laserMode;
      laserModeIndicator.classList.toggle("on", dynamic);
      const hint = t(dynamic ? "html.spindle.dynamicPower" : "html.spindle.constantPower");
      laserModeIndicator.title = hint;
      laserModeIndicator.setAttribute("aria-label", hint);
    }
    // Use power terminology only when this machine's configuration advertises a laser.
    const profileSelected = Boolean(machineData.profile);
    const laserOutput = laserMode;
    const unitsLabel = currentPowerUnit ? ` ${currentPowerUnit}` : "";
    const powerLabel = `${t("html.spindle.power")}${unitsLabel}`;
    valueLabel.textContent = laserOutput ? powerLabel : t("html.spindle.rpm");
    document.querySelector("#spindle-manual-speed-label")?.replaceChildren(document.createTextNode(laserOutput ? t("html.spindle.customPower", { unit: currentPowerUnit }) : t("html.spindle.customSpeed")));
    stepButton.title = laserOutput ? t("html.spindle.powerPresetTitle", { unit: currentPowerUnit }) : t("html.spindle.presetTitle");
    laserModeIndicator?.classList.toggle("profile-laser-mode", laserMode && !machineData.controllerLaser);
    manualSpeedInput.setAttribute(
      "aria-label",
      laserMode ? t("html.spindle.power") : t("html.spindle.manualAria"),
    );
    speedSlider.dataset.i18nTitle = laserMode
      ? "html.spindle.powerSliderTitle"
      : "html.spindle.sliderTitle";
    speedSlider.max = String(laserMode ? laserPowerMaximum : maximumRpm);
    speedSlider.min = String(laserMode ? minimumPower : minimumRpm);
    manualSpeedInput.min = speedSlider.min;
    manualSpeedInput.max = speedSlider.max;
    clockwiseButton.hidden = laserMode;
    counterClockwiseButton.hidden = laserMode;
    if (laserToggle) laserToggle.hidden = !laserMode;
    panel?.classList.toggle("laser-active", laserMode && spindleRunning);
    panel?.classList.toggle("laser-mode", laserMode);
    panel?.classList.toggle("profile-laser-only", profileSelected && machineData.profile?.machineType === "laser");

    speedSlider.min = String(laserMode ? minimumPower : minimumRpm);
    speedSlider.max = String(laserMode ? laserPowerMaximum : maximumRpm);
    speedSlider.step = "any";

    const reportedSetpoint = laserMode && status.laser_power != null
      ? Number(status.laser_power)
      : Number(status.spindle_commanded_rpm);
    const currentSetpoint = Number(speedSlider.value);
    const outputMinimum = laserMode ? minimumPower : minimumRpm;
    const outputMaximum = laserMode ? laserPowerMaximum : maximumRpm;
    const nextSetpoint =
      spindleRunning && Number.isFinite(reportedSetpoint) && reportedSetpoint > 0
        ? reportedSetpoint
        : Number.isFinite(currentSetpoint) && currentSetpoint >= outputMinimum
          ? Math.min(currentSetpoint, outputMaximum)
          : outputMinimum;
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
    if (laserToggle) {
      laserToggle.hidden = !laserMode;
      const laserLabel = direction === "cw" ? t("spindle.laserStop") : t("spindle.laserToggle");
      laserToggle.title = laserLabel;
      laserToggle.setAttribute("aria-label", laserLabel);
      laserToggle.classList.toggle("running", direction === "cw");
    }
    speedSlider.disabled =
      !status.connected ||
      outputMaximum <= 0 ||
      spindleRunning ||
      direction !== "stopped";
  }

  return {
    update,
    setMachineProfile(profile) {
      machineData.profile = profile;
      laserMode = profile?.machineType === "laser" || (profile?.machineType !== "mill" && machineData.controllerLaser);
      if (laserModeIndicator) laserModeIndicator.hidden = !laserMode;
      if (profile?.machineType === "laser") {
        clockwiseButton.hidden = true;
        if (laserToggle) laserToggle.hidden = false;
        counterClockwiseButton.hidden = true;
        valueLabel.textContent = `${t("html.spindle.power")}${currentPowerUnit ? ` ${currentPowerUnit}` : ""}`;
        document.querySelector("#spindle-manual-speed-label")?.replaceChildren(document.createTextNode(t("html.spindle.customPower", { unit: currentPowerUnit })));
        stepButton.title = t("html.spindle.powerPresetTitle", { unit: currentPowerUnit });
        panel?.classList.add("laser-mode", "profile-laser-only");
        laserModeIndicator?.classList.add("profile-laser-only");
      } else {
        laserModeIndicator?.classList.remove("on");
        clockwiseButton.hidden = false;
        counterClockwiseButton.hidden = false;
        if (laserToggle) laserToggle.hidden = !laserMode;
        valueLabel.textContent = t("html.spindle.rpm");
        document.querySelector("#spindle-manual-speed-label")?.replaceChildren(document.createTextNode(t("html.spindle.customSpeed")));
        stepButton.title = t("html.spindle.presetTitle");
        panel?.classList.remove("profile-laser-only");
        laserModeIndicator?.classList.remove("profile-laser-only");
      }
    },
    get commandError() {
      return commandError;
    },
  };
}
