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
  const connectionInfo = document.querySelector("#connection-info");
  const axisRowsContainer = document.querySelector("#axis-rows");
  const coordinateSystem = document.querySelector("#coordinate-system");

  // Telemetry strip under the Zero all / Home all buttons (task: show the
  // current motion speed with mode indication and the spindle RPM from the
  // status telemetry, plus the coolant states).
  const telemetry = {
    root: document.querySelector("#position-telemetry"),
    feed: document.querySelector("#telemetry-feed"),
    mode: document.querySelector("#telemetry-mode"),
    spindle: document.querySelector("#telemetry-spindle"),
    spindleCaption: document.querySelector("#telemetry-spindle-caption"),
    mist: document.querySelector("#coolant-mist"),
    flood: document.querySelector("#coolant-flood"),
  };

  /**
   * Mirrors the speed/mode/spindle/coolant telemetry of one status payload
   * into the strip below the position actions.
   *
   * The strip is hidden while disconnected so stale numbers never linger
   * after a link drop. `movement_speed` already prefers the jog feed over
   * the program feed on the backend; `movement_mode` carries the modal group
   * the controller reports (Jog / G0 / G1 / G2 / G3 / Homing / G-code).
   * Coolant lamps light only while the corresponding bit is on.
   *
   * @param {object} status - Parsed `/api/status` payload.
   * @param {boolean} connected - Live connection flag of the same poll.
   * @returns {void}
   */
  function updateTelemetry(status, connected) {
    if (!telemetry.root) return;
    telemetry.root.hidden = !connected;
    if (!connected) return;
    const speed = Number(status.movement_speed);
    telemetry.feed.textContent = Number.isFinite(speed) ? `${speed.toFixed(0)}` : "—";
    telemetry.mode.textContent = status.movement_mode || "—";
    const profileType = app.getMachineProfile?.()?.machineType;
    const laserMode = profileType === "laser" || (profileType !== "mill" && (status.spindle_mode === "laser_constant" || status.spindle_mode === "laser_dynamic"));
    const output = Number(laserMode ? status.laser_power : status.spindle_rpm);
    const modeIndicator = document.querySelector("#laser-mode-indicator");
    if (modeIndicator) {
      modeIndicator.hidden = !laserMode;
      const dynamic = status.spindle_mode === "laser_dynamic";
      modeIndicator.classList.toggle("on", dynamic);
      const modeTitle = t(dynamic ? "html.spindle.dynamicPower" : "html.spindle.constantPower");
      modeIndicator.title = modeTitle;
      modeIndicator.setAttribute("aria-label", modeTitle);
    }
    telemetry.spindle.textContent = Number.isFinite(output) ? `${output.toFixed(0)}` : "—";
    if (telemetry.spindleCaption) telemetry.spindleCaption.textContent = t(laserMode ? "html.position.telemetryLaser" : "html.position.telemetrySpindle");
    telemetry.root.classList.toggle("laser-telemetry", laserMode);
    telemetry.mist.classList.toggle("on", Boolean(status.mist_coolant));
    telemetry.flood.classList.toggle("on", Boolean(status.flood_coolant));
  }

  /**
   * Displays the active controller identity and endpoint beside the connection chip.
   * Clear the metadata on disconnect so a stale endpoint is never mistaken for live.
   * @param {object|null|undefined} info - Active connection metadata from the backend.
   * @param {boolean} connected - Whether the current status poll has a live link.
   * @returns {void}
   */
  function showRecoveryNotice(kind, status) {
    const notice = document.querySelector("#recovery-notice");
    if (!notice) return;
    notice.hidden = false;
    notice.dataset.kind = kind;
    notice.replaceChildren();
    const message = document.createElement("span");
    message.textContent = kind === "connection_lost"
      ? "Connection lost. Machine position, program line and spindle/laser output are unknown. Reconnect and inspect the machine; automatic resume is disabled."
      : `Reconnected: current controller state is ${status.state}; cached program line ${status.current_line} is not proof that continuation is safe. Reload or restart from the beginning only after operator review.`;
    notice.append(message);
    if (kind === "reconnected") {
      const acknowledge = document.createElement("button");
      acknowledge.id = "recovery-reconnect";
      acknowledge.type = "button";
      acknowledge.textContent = "Acknowledge";
      acknowledge.addEventListener("click", () => { notice.hidden = true; });
      notice.append(acknowledge);
    }
  }

  /** Updates the conservative reconnect notice from one newly polled live status sample. */
  function updateRecoveryNotice(status) {
    const notice = document.querySelector("#recovery-notice");
    if (!notice) return;
    const previous = latestStatus;
    if (previous?.connected === true && status.connected === false) {
      showRecoveryNotice("connection_lost", previous);
    } else if (previous?.connected === false && status.connected === true) {
      showRecoveryNotice("reconnected", status);
    }
  }

  /** Displays the active controller identity without making stale endpoint data appear live. */
  function updateConnectionInfo(info, connected) {
    if (!connectionInfo) return;
    if (!connected || !info) {
      connectionInfo.textContent = "";
      connectionInfo.title = "";
      connectionInfo.hidden = true;
      return;
    }
    const identity = info.machine_name || info.protocol || "CNC";
    const details = [identity, info.transport, info.address].filter(Boolean);
    connectionInfo.textContent = details.join(" · ");
    connectionInfo.title = details.join(" — ");
    connectionInfo.hidden = false;
  }

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
      const previousStatus = latestStatus;
      machineConnected = Boolean(status.connected);
      if (previousStatus?.connected === true && !machineConnected) {
        showRecoveryNotice("connection_lost", previousStatus);
      } else if (previousStatus?.connected === false && machineConnected) {
        showRecoveryNotice("reconnected", status);
      }
      latestStatus = status;
      machineState = status.state || "DISCONNECTED";
      connection.textContent = machineConnected ? t("ui.connected") : t("ui.noLink");
      updateConnectionInfo(status.connection_info, machineConnected);
      updateRecoveryNotice(status);
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
      // Per-axis feed controls (task) mirror the same axis-presence rule.
      app.jogDial.axisSpeeds?.render(status.configuration);
      app.positionAxisView.updateDisplay();
      coordinateSystem.value = status.coordinate_system || "g54";
      updateTelemetry(status, machineConnected);

      app.messagePanel.refresh(status);
      app.positionReadouts(status);

      document.querySelector("#message").textContent =
        status.error || app.spindleControls.commandError || status.message || t("ui.controllerReady");
      const recoveryNotice = document.querySelector("#recovery-notice");
      if (recoveryNotice?.dataset.kind === "reconnected") {
        document.querySelector("#recovery-reconnect")?.remove();
      }
      app.scene.applyStatus(status);
    } catch (error) {
      machineConnected = false;
      showRecoveryNotice("connection_lost", null);
      app.scene.clearTool();
      connection.textContent = t("ui.connectionError");
      updateConnectionInfo(null, false);
      connectionDot.classList.remove("online");
      connectionDot.classList.add("offline");
      connectionChip.classList.remove("connection-online");
      connectionChip.classList.add("connection-offline");
      state.textContent = translateState("DISCONNECTED");
      machineState = "DISCONNECTED";
      document.querySelector("#message").textContent = error.message;
      setJogEnabled(false);
      updateTelemetry({}, false);
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
