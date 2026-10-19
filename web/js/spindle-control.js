import { invokeOrFetch } from "./api.js";

export function initSpindleControls({ refreshStatus }) {
  const speedSlider = document.querySelector("#spindle-speed-slider");
  const clockwiseButton = document.querySelector("#spindle-cw");
  const counterClockwiseButton = document.querySelector("#spindle-ccw");
  const stepButton = document.querySelector("#spindle-step");
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
        `Введите скорость от ${minimumRpm.toFixed(0)} до ${maximumRpm.toFixed(0)} RPM.`,
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
    const nextDirection = direction === requestedDirection.toLowerCase()
      ? "stopped"
      : requestedDirection.toLowerCase();
    const enabled = nextDirection !== "stopped";
    const rpm = Number(speedSlider.value);

    pendingDirection = nextDirection;
    direction = nextDirection;
    updateButtons();

    try {
      await invokeOrFetch(
        "set_spindle",
        { rpm, enabled, direction: requestedDirection },
        "/api/spindle",
        "POST",
      );
      commandError = null;
      await refreshStatus();
    } catch (error) {
      pendingDirection = null;
      commandError = `Ошибка шпинделя: ${error.message}`;
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
    // Не заменяем SVG при каждом обновлении телеметрии: pointerdown/click должны
    // оставаться на одном DOM-элементе, иначе браузер может отменить click.
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

    clockwiseButton.setAttribute(
      "aria-label",
      clockwiseActive ? "Остановить шпиндель" : "Запустить шпиндель по часовой стрелке",
    );
    counterClockwiseButton.setAttribute(
      "aria-label",
      counterClockwiseActive
        ? "Остановить шпиндель"
        : "Запустить шпиндель против часовой стрелки",
    );
    clockwiseButton.title = clockwiseActive
      ? "Остановить шпиндель"
      : "Запустить шпиндель по часовой стрелке";
    counterClockwiseButton.title = counterClockwiseActive
      ? "Остановить шпиндель"
      : "Запустить шпиндель против часовой стрелки";
  }

  function update(status) {
    spindleRunning = Boolean(status.spindle_enabled);
    minimumRpm = Math.max(0, Number(status.spindle_min_rpm) || 0);
    maximumRpm = Math.max(0, Number(status.spindle_max_rpm) || 0);

    speedSlider.min = String(minimumRpm);
    speedSlider.max = String(maximumRpm);
    speedSlider.step = "any";

    const reportedSetpoint = Number(status.spindle_commanded_rpm);
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
