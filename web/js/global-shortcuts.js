import { eventChord, isLayoutIndependent, loadShortcutMap } from "./shortcut-settings.js";

/**
 * Registers the configurable global keyboard action handlers.
 * JOG keys remain handled by their existing focus-aware module and are not redefined here.
 * @param {{getProgram: Function, getMachineState: Function, refreshStatus: Function, scene: object}} app - Shared UI state.
 * @returns {void}
 */
export function initGlobalShortcuts(app) {
  let bindings = loadShortcutMap();
  let layoutIndependent = isLayoutIndependent();
  window.addEventListener("cnc-shortcuts-changed", (event) => {
    bindings = event.detail?.bindings ?? loadShortcutMap();
    layoutIndependent = event.detail?.layoutIndependent ?? isLayoutIndependent();
  });

  /** Executes the selected action through the existing program API or local view state. */
  async function execute(action) {
    if (action === "run") {
      if (!app.getProgram()) return;
      if (app.getMachineState() === "PAUSED") {
        await fetch("/api/program/resume", { method: "POST" });
      } else {
        // Delegate Run to the same preflight/confirmation flow as the visible control.
        document.querySelector("#job-run")?.click();
        return;
      }
      await app.refreshStatus();
    } else if (action === "pause") {
      await fetch("/api/program/pause", { method: "POST" });
      await app.refreshStatus();
    } else if (action === "abort") {
      await fetch("/api/program/abort", { method: "POST" });
      await app.refreshStatus();
    } else if (action === "viewerReset") {
      Object.assign(app.scene.camera, { zoom: 1, panX: 0, panY: 0 });
      app.scene.draw();
    } else if (action === "toggleFiles") {
      document.querySelector("#files-tab")?.click();
    } else if (action === "togglePosition") {
      document.querySelector(".position-panel-heading")?.closest(".panel")?.classList.toggle("shortcut-panel-collapsed");
    } else if (action === "toggleJog") {
      document.querySelector("#jog-panel")?.classList.toggle("shortcut-panel-collapsed");
    } else if (action === "toggleViewer") {
      document.querySelector("#viewer-panel")?.classList.toggle("shortcut-panel-collapsed");
    } else if (action === "toggleMacros") {
      document.querySelector("#macros-tab")?.click();
    } else if (action === "toggleConsole") {
      document.querySelector("#commands-tab")?.click();
    }
  }

  window.addEventListener("keydown", (event) => {
    const target = event.target;
    const editing = target instanceof HTMLElement && (target.isContentEditable || target.closest("input,textarea,select,[contenteditable='true']"));
    const chord = eventChord(event, layoutIndependent);
    if (!chord) return;
    const action = Object.entries(bindings).find(([, key]) => key.toLowerCase() === chord.toLowerCase())?.[0];
    if (!action || (editing && action !== "abort")) return;
    event.preventDefault();
    void execute(action).catch((error) => {
      const note = document.querySelector("#job-note");
      if (note) note.textContent = error.message;
    });
  });
}
