// Status panel controller: polls the backend status endpoint, mirrors the
// snapshot into the header chips, drives every dependent UI handle (jog
// enablement, job buttons, spindle, axis rows, message lines, position
// readouts, viewer) and owns the connection/machine-state globals that the
// rest of the app reads through the shared `app` bag. Extracted verbatim
// from main.js; cross-feature calls are resolved lazily via `app` because
// the panel is created before the jog/status handles are attached to it.
import { t } from "./i18n.js";

/**
 * Creates the status panel and returns its accessors plus the poller.
 *
 * @param {object} app - Shared application bag. The panel fills
 *   `app.getStatus/getConnected/getMachineState` itself and reads
 *   `app.clock`, `app.messagePanel`, `app.jobControls`,
 *   `app.spindleControls`, `app.positionAxisView`, `app.positionReadouts`,
 *   `app.jogDial`, `app.jog` and `app.scene` lazily inside `refreshStatus`.
 * @returns {object} Status handle: `getStatus`, `getConnected`,
 *   `getMachineState`, `refreshStatus`, `setJogEnabled`.
 */
export function createStatusPanel(app) {
  const connection = document.querySelector("#connection");
  const connectionChip = document.querySelector(".connection-chip");
  const connectionDot = document.querySelector("#connection-dot");
  const state = document.querySelector("#state");
  const axisRowsContainer = document.querySelector("#axis-rows");
  const coordinateSystem = document.querySelector("#coordinate-system");

  let latestStatus = null;
  let machineConnected = false;
  let statusRequestInFlight = false;
  let machineState = "DISCONNECTED";

  async function requestStatus() {
    const invoke = window.__TAURI__?.core?.invoke;
    if (invoke) return invoke("get_status");

    const response = await fetch("/api/status", { cache: "no-store" });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    return response.json();
  }

  /**
   * Enables or disables every machine-control affordance depending on the
   * connection state. Called with the freshly computed `machineConnected` at
   * the tail of each successful poll, with `false` on errors and at startup.
   * @param {boolean} enabled - True while the backend reports a live machine.
   */
  function setJogEnabled(enabled) {
    app.jog.jogButtons.forEach((button) => {
      button.disabled = !enabled;
    });
    document.querySelectorAll("[data-dial-axis], [data-dial-continuous-axis]").forEach((button) => {
      button.disabled = !enabled;
    });
    axisRowsContainer.querySelectorAll("[data-zero-axis], [data-home-axis]").forEach((button) => {
      button.disabled = !enabled;
    });
    document.querySelector("#zero-all").disabled = !enabled;
    document.querySelector("#home-all").disabled = !enabled;
    document.querySelectorAll("[data-jog-stop]").forEach((button) => {
      button.disabled = !enabled;
    });
    coordinateSystem.disabled = !enabled;
  }

  /**
   * Maps raw backend state tokens (IDLE, HOMING, RUNNING, ...) to localized state
   * chip labels. Unknown tokens fall back to the raw value, and a missing state
   * renders as an em dash, so new controller states never surface as a
   * missing-translation key.
   *
   * @param {string|undefined} rawState - status.state value from the backend.
   * @returns {string} State chip text in the active locale.
   */
  function translateState(rawState) {
    if (!rawState) return "—";
    const key = {
      IDLE: "ui.state.idle",
      HOMING: "ui.state.homing",
      READING: "ui.state.reading",
      RUNNING: "ui.state.running",
      PAUSED: "ui.state.paused",
      ALARM: "ui.state.alarm",
      DISCONNECTED: "ui.state.disconnected",
    }[rawState];
    return key ? t(key) : rawState;
  }

  async function refreshStatus() {
    if (statusRequestInFlight) return;
    statusRequestInFlight = true;

    try {
      const status = await requestStatus();
      latestStatus = status;
      machineConnected = Boolean(status.connected);
      machineState = status.state || "DISCONNECTED";
      connection.textContent = machineConnected ? t("ui.connected") : t("ui.noLink");
      app.clock.update(status);
      connectionDot.classList.toggle("online", machineConnected);
      connectionDot.classList.toggle("offline", !machineConnected);
      connectionChip.classList.toggle("connection-online", machineConnected);
      connectionChip.classList.toggle("connection-offline", !machineConnected);
      state.textContent = translateState(status.state);
      // Assign (not addEventListener) so the handler is not re-added on every poll.
      state.onclick = async (event) => {
        if (machineState !== "ALARM") return;

        const menu = document.createElement("div");
        menu.style.position = "absolute";
        menu.style.top = `${event.clientY}px`;
        menu.style.left = `${event.clientX}px`;
        menu.style.background = "var(--surface)";
        menu.style.border = "1px solid var(--border)";
        menu.style.borderRadius = "7px";
        menu.style.padding = "10px";
        menu.style.zIndex = "1000";
        menu.style.boxShadow = "0 4px 10px rgba(0,0,0,0.3)";

        const btnReset = document.createElement("button");
        btnReset.textContent = t("ui.alarm.reset");
        btnReset.style.display = "block";
        btnReset.style.width = "100%";
        btnReset.style.marginBottom = "5px";
        btnReset.addEventListener("click", async () => {
          try {
            await fetch("/api/reset-alarm", { method: "POST" });
            await refreshStatus();
          } catch (error) {
            document.querySelector("#message").textContent = t("common.errorPrefix", { message: error.message });
          } finally {
            menu.remove();
          }
        });

        const btnReboot = document.createElement("button");
        btnReboot.textContent = t("ui.alarm.reboot");
        btnReboot.style.display = "block";
        btnReboot.style.width = "100%";
        btnReboot.addEventListener("click", async () => {
          try {
            await fetch("/api/command", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ command: "$RST=*" })
              });
              await refreshStatus();
            } catch (error) {
              document.querySelector("#message").textContent = t("common.errorPrefix", { message: error.message });
          } finally {
            menu.remove();
          }
        });

        menu.appendChild(btnReset);
        menu.appendChild(btnReboot);
        document.body.appendChild(menu);

        const closeMenu = (e) => {
          if (!menu.contains(e.target)) {
            menu.remove();
            document.removeEventListener("click", closeMenu);
          }
        };
        setTimeout(() => document.addEventListener("click", closeMenu), 0);
      };
      const stateClass = ({
        IDLE: "state-idle",
        HOMING: "state-homing",
        READING: "state-running",
        RUNNING: "state-running",
        PAUSED: "state-paused",
        ALARM: "state-alarm",
        DISCONNECTED: "state-disconnected",
      })[machineState] || "";
      state.classList.remove(
        "state-idle",
        "state-homing",
        "state-running",
        "state-paused",
        "state-alarm",
        "state-disconnected",
      );
      if (stateClass) state.classList.add(stateClass);
      setJogEnabled(machineConnected);
      app.jobControls.update(status);
      app.spindleControls.update(status);
      // Axis rows in the POSITION panel and the rotary jog columns share the
      // same machine configuration payload.
      app.positionAxisView.render(status.configuration);
      app.jogDial.rotaryColumns.render(status.configuration);
      app.positionAxisView.updateDisplay();
      coordinateSystem.value = status.coordinate_system || "g54";

      app.messagePanel.refresh(status);
      app.positionReadouts(status);

      document.querySelector("#message").textContent =
        status.error || app.spindleControls.commandError || status.message || t("ui.controllerReady");
      app.scene.applyStatus(status);
    } catch (error) {
      machineConnected = false;
      app.scene.clearTool();
      connection.textContent = t("ui.connectionError");
      connectionDot.classList.remove("online");
      connectionDot.classList.add("offline");
      connectionChip.classList.remove("connection-online");
      connectionChip.classList.add("connection-offline");
      state.textContent = translateState("DISCONNECTED");
      machineState = "DISCONNECTED";
      document.querySelector("#message").textContent = error.message;
      setJogEnabled(false);
      app.jobControls.update({ connected: false, total_lines: 0, state: "DISCONNECTED" });
    } finally {
      statusRequestInFlight = false;
    }
  }

  // Publish the accessors on the shared bag so every feature reads the single
  // authoritative copy of the connection/status state.
  app.getStatus = () => latestStatus;
  app.getConnected = () => machineConnected;
  app.getMachineState = () => machineState;

  return {
    getStatus: () => latestStatus,
    getConnected: () => machineConnected,
    getMachineState: () => machineState,
    refreshStatus,
    setJogEnabled,
  };
}
