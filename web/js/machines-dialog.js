// Discovered-machines dialog controller: renders the machine list returned by
// the backend discovery endpoints (`GET/POST /api/discovered-machines`), lets
// the user switch the active machine (`POST /api/machines/select`), and wires
// the dialog's open/refresh/close controls. Extracted verbatim from main.js.
import { t } from "./i18n.js";

/**
 * Creates the machines dialog and attaches its DOM listeners.
 *
 * @param {object} deps - Injected dependencies.
 * @param {() => Promise<void>} deps.refreshStatus - Re-pulls the machine
 *   status after a successful switch so the status panel immediately tracks
 *   the newly selected device.
 * @returns {{ openMachinesDialog: () => Promise<void> }} Dialog handle.
 */
export function initMachinesDialog({ refreshStatus }) {
  const machinesDialog = document.querySelector("#machines-dialog");
  const machinesList = document.querySelector("#machines-list");
  const machinesRefreshButton = document.querySelector("#machines-refresh");
  // True while a machine-switch request (POST /api/machines/select) is running;
  // guards against double clicks and concurrent switches in the machines dialog.
  let switchingMachine = false;

  function renderDiscoveredMachines(payload) {
    machinesList.replaceChildren();
    const machines = Array.isArray(payload?.machines) ? payload.machines : [];
    if (!machines.length) {
      const empty = document.createElement("p");
      empty.className = "machines-empty";
      empty.textContent = t("ui.machines.empty");
      machinesList.append(empty);
      return;
    }
    for (const machine of machines) {
      // Each row is a button: clicking it asks the backend to switch the active
      // machine adapter to this device (POST /api/machines/select). The already
      // connected row stays visible but disabled, and the list is frozen while
      // a switch is in flight.
      const row = document.createElement("button");
      row.type = "button";
      row.className = `machine-entry${machine.connected ? " connected" : ""}`;
      row.dataset.machineId = machine.id || "";
      row.disabled = Boolean(machine.connected);
      row.title = machine.connected
        ? t("ui.connected")
        : t("ui.machines.connectTitle", { address: machine.address || machine.id });
      const details = document.createElement("div");
      details.className = "machine-entry-details";
      const endpoint = document.createElement("span");
      endpoint.className = "machine-entry-endpoint";
      endpoint.textContent = machine.address;
      const system = document.createElement("span");
      system.className = "machine-entry-meta";
      const identity = [machine.protocol, machine.version].filter(Boolean).join(" ");
      const machineName = String(machine.machine_name || "").trim();
      const endpointName = String(machine.address || "").split(/[/:]/).filter(Boolean).pop();
      const transportName = machine.transport === "serial"
        ? (endpointName || "serial")
        : machine.transport || "";
      system.textContent = [machineName, identity, transportName].filter(Boolean).join(" · ") || t("ui.machines.unidentified");
      // Dedicated `machine-entry-state` class marks the status label ("Connected"
      // / "Discovered") so connectDiscoveredMachine can find it reliably. The
      // system meta span above shares `.machine-entry-meta`, so a plain
      // `.machine-entry-meta:last-child` query would wrongly match it first.
      const label = document.createElement("span");
      label.className = "machine-entry-meta machine-entry-state";
      label.textContent = machine.connected ? t("ui.connected") : t("ui.machines.discovered");
      details.append(endpoint, system);
      row.append(details, label);
      machinesList.append(row);
    }
  }

  /**
   * Switches the backend to the machine row the user clicked. All rows and the
   * refresh button are disabled while the request is in flight; on success the
   * returned machine list re-renders (the new machine shows as connected) and
   * the status panel refreshes so positions and state come from the new device.
   * @param {HTMLElement} button - The `.machine-entry` button that was clicked.
   */
  async function connectDiscoveredMachine(button) {
    const machineId = button.dataset.machineId;
    if (!machineId || switchingMachine) return;
    switchingMachine = true;
    const entries = [...machinesList.querySelectorAll(".machine-entry")];
    for (const entry of entries) entry.disabled = true;
    machinesRefreshButton.disabled = true;
    // targets the status label exclusively (see renderDiscoveredMachines)
    const label = button.querySelector(".machine-entry-state");
    const restoreLabel = label ? label.textContent : "";
    if (label) label.textContent = t("ui.machines.connecting");
    try {
      const response = await fetch("/api/machines/select", {
        method: "POST",
        headers: { "content-type": "application/json" },
        cache: "no-store",
        body: JSON.stringify({ id: machineId }),
      });
      if (!response.ok) {
        const details = await response.text();
        throw new Error(details || `HTTP ${response.status}`);
      }
      // The backend answers with the full machine list including the freshly
      // connected machine, so the dialog updates without another round trip.
      renderDiscoveredMachines(await response.json());
      // Pull the status of the newly selected machine immediately; the regular
      // status poller then keeps tracking it.
      await refreshStatus();
    } catch (error) {
      // Restore the previous rendering and surface the failure inside the list.
      if (label) label.textContent = restoreLabel;
      for (const entry of entries) {
        entry.disabled = entry.classList.contains("connected");
      }
      machinesList.replaceChildren();
      const message = document.createElement("p");
      message.className = "machines-empty";
      message.textContent = t("ui.machines.connectError", { message: error.message });
      machinesList.append(message);
    } finally {
      switchingMachine = false;
      machinesRefreshButton.disabled = false;
    }
  }

  async function loadDiscoveredMachines({ refresh = false } = {}) {
    machinesRefreshButton.disabled = refresh;
    if (refresh) machinesRefreshButton.classList.add("searching");
    machinesList.replaceChildren();
    const loading = document.createElement("p");
    loading.className = "machines-empty";
    loading.textContent = refresh ? t("ui.machines.searching") : t("ui.machines.loading");
    machinesList.append(loading);
    try {
      const response = await fetch("/api/discovered-machines", {
        method: refresh ? "POST" : "GET",
        cache: "no-store",
      });
      if (!response.ok) {
        const details = await response.text();
        throw new Error(details || `HTTP ${response.status}`);
      }
      if (!refresh) {
        renderDiscoveredMachines(await response.json());
        return;
      }

      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const events = buffer.split("\n\n");
        buffer = events.pop() || "";
        for (const rawEvent of events) {
          const data = rawEvent.split("\n")
            .filter((line) => line.startsWith("data:"))
            .map((line) => line.slice(5).trim())
            .join("\n");
          if (data) renderDiscoveredMachines(JSON.parse(data));
        }
      }
    } catch (error) {
      machinesList.replaceChildren();
      const message = document.createElement("p");
      message.className = "machines-empty";
      message.textContent = t("ui.machines.loadError", { message: error.message });
      machinesList.append(message);
    } finally {
      machinesRefreshButton.disabled = false;
      machinesRefreshButton.classList.remove("searching");
    }
  }

  async function openMachinesDialog() {
    machinesDialog.showModal();
    await loadDiscoveredMachines();
  }

  document.querySelector("#connection-machines-button").addEventListener("click", openMachinesDialog);
  machinesRefreshButton.addEventListener("click", () => loadDiscoveredMachines({ refresh: true }));
  // Delegated click handler: machine rows are re-rendered constantly during
  // discovery streams, so the listener lives on the stable list container.
  machinesList.addEventListener("click", (event) => {
    const button = event.target.closest("button.machine-entry");
    if (!button || button.disabled) return;
    connectDiscoveredMachine(button);
  });
  document.querySelector("#machines-dialog-close").addEventListener("click", () => machinesDialog.close());
  machinesDialog.addEventListener("click", (event) => {
    if (event.target === machinesDialog) machinesDialog.close();
  });

  return { openMachinesDialog };
}
