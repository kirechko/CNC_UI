// Main UI entry point: the composition root. Creates the shared mutable `app`
// bag, instantiates every feature controller (status polling, viewer stage,
// program panel, the JOG stack, machines dialog, message panel, clocks and
// leaf control handles), wires the global window listeners and runs the
// startup sequence. All feature logic lives in the sibling modules; this file
// only fixes creation order and late-bound cross-feature handles. Every
// cross-module read goes through a closure or getter on `app`, so modules may
// be created in any order as long as the bag is complete before the first
// event fires or the first status poll runs (all of that happens at the very
// bottom of this file).
import { applyTranslations, loadLocale, resolveRequestedLocale, t } from "./i18n.js";
import { initUiMessageLogger } from "./ui-message-logger.js";
import { initSidebarTabs } from "./sidebar.js";
import { initFileManager } from "./file-manager-ui.js";
import { initPositionControls } from "./position-controls.js";
import { createPositionReadoutUpdater } from "./position-readouts.js";
import { initPositionAxisView } from "./position-axis-view.js";
import { initJobControls } from "./job-control.js";
import { initSpindleControls } from "./spindle-control.js";
import { initCommandConsole } from "./command-console.js";
import { logMachineConsoleEvent, logMachineConsoleError } from "./machine-console-log.js";
import { initMachinesDialog } from "./machines-dialog.js";
import { initMessageLogPanel } from "./message-log-panel.js";
import { initToolpathClock } from "./toolpath-clock.js";
import { initSceneStage } from "./scene-stage.js";
import { initProgramPanel } from "./program-panel.js";
import { createJogContext } from "./jog-context.js";
import { initJogMotion } from "./jog-motion.js";
import { initJogDial } from "./jog-dial.js";
import { createStatusPanel } from "./status-panel.js";
import { initLayoutSplitters } from "./layout-splitters.js";
import { initLayoutRowSplitters } from "./layout-row-splitters.js";
import { initTooltips } from "./tooltips.js";
import { initCompactLabels } from "./compact-labels.js";
import { initUiSettings } from "./ui-settings.js";

// Load the requested locale (?lang=xx, default en) before any UI text is shown.
// English is always loaded as the fallback dictionary inside loadLocale().
await loadLocale(resolveRequestedLocale());
applyTranslations(document);

// Make the column boundaries draggable and restore the saved layout.
initLayoutSplitters();

// Make the panel boundaries inside the left and right columns draggable.
initLayoutRowSplitters();

// Replace native title tooltips with the styled bubble that matches the JOG Z buttons.
initTooltips();

// Swap wordy captions ("Zero all"/"Home all") for their short variants whenever
// the POSITION panel is too narrow to keep them on a single line. The handle is
// kept so a locale switch can re-evaluate the shortened captions.
const compactLabels = initCompactLabels({ root: document });

// Open the interface settings dialog from the header gear button. The theme
// itself was already applied before first paint by the inline <head> script;
// this module only handles the dialog and live theme switching.
initUiSettings();

// The shared application bag. Controllers fill their own slice as the script
// runs top-to-bottom; everything else reads through lazy getters/closures.
const app = {};

// Mirror uncaught UI errors and console warnings into the backend message log.
// `getStatus` resolves lazily because the status panel is created further
// below, long before any loggable event can fire.
initUiMessageLogger({
  elements: [
    [document.querySelector("#message"), "interface"],
    [document.querySelector("#job-note"), "program control"],
  ],
  getStatus: () => app.getStatus(),
});

// Toolpath remaining-time clock (viewer footer). Reads program and status
// from the bag on every update.
app.clock = initToolpathClock(app);

// 3D viewer stage: scene camera, tool animation, stock, orientation cube,
// camera controls and the toolbar. Reads status/connection/program lazily.
app.scene = initSceneStage(app);

// Program panel: owns the loaded program and the load/edit entry points.
// Calls into `app.clock` and `app.scene`, both already created above.
app.program = initProgramPanel(app);
app.getProgram = () => app.program.getProgram();

// Shared JOG state (steps, speeds, motion mode, continuous bookkeeping, refs
// and the jog API handle). The API's connection guard reads the bag lazily.
app.jog = createJogContext(app);

// Status panel: the polling loop and the connection/machine-state accessors.
// Assigns app.getStatus/getConnected/getMachineState on creation and calls
// into every other handle lazily inside refreshStatus.
app.status = createStatusPanel(app);
app.refreshStatus = () => app.status.refreshStatus();

// Message-log widgets (footer latest lines + full log dialog with calendar).
app.messagePanel = initMessageLogPanel();

// Discovered-machines dialog; switches the active backend adapter.
initMachinesDialog({ refreshStatus: () => app.refreshStatus() });

// Job control buttons (run/pause/abort) driven from status + program state.
app.jobControls = initJobControls({
  getProgram: () => app.getProgram(),
  getMachineState: () => app.getMachineState(),
  refreshStatus: () => app.refreshStatus(),
});

// MDI command console with history and autocomplete.
const commandConsole = initCommandConsole({
  logEvent: logMachineConsoleEvent,
  logError: logMachineConsoleError,
  refreshStatus: () => app.refreshStatus(),
  isConnected: () => app.getConnected(),
});

// Work-position zero/home dialog controls (single-axis + all-axes actions).
const positionControls = initPositionControls({
  getStatus: () => app.getStatus(),
  refreshStatus: () => app.refreshStatus(),
});

// Axis rows of the POSITION panel: built from the machine configuration and
// updated on every poll. Zero/home actions delegate to positionControls.
app.positionAxisView = initPositionAxisView({
  container: document.querySelector("#axis-rows"),
  workToggle: document.querySelector("#wpos-toggle"),
  machineToggle: document.querySelector("#mpos-toggle"),
  openPosition: (axis) => positionControls.openDialog(axis),
  async zeroAxis(axis) {
    await positionControls.sendPositionAction(
      "zero_axis",
      "/api/position/zero-axis",
      { axis },
    );
    await app.refreshStatus();
  },
  async homeAxis(axis) {
    await positionControls.sendPositionAction(
      "home_axis",
      "/api/position/home",
      { axis },
    );
    document.querySelector("#message").textContent = t("main.homingStarted", { axis });
  },
  showError(message) {
    document.querySelector("#message").textContent = message;
  },
});

// Numeric work/machine readouts for every axis row.
app.positionReadouts = createPositionReadoutUpdater(document.querySelector("#axis-rows"));

// Spindle speed/direction controls.
app.spindleControls = initSpindleControls({ refreshStatus: () => app.refreshStatus() });

// JOG motion core: incremental jog, continuous hold-to-move and stop-all.
app.jogMotion = initJogMotion(app, app.jog);

// JOG dial: canvas rendering, hover/hit-test, mode + fit controls, rotary
// columns, speed/step dialog. Repaints the dial after both fits settle.
app.jogDial = initJogDial(app, app.jog, app.jogMotion);

// Files/messages sidebar tabs.
initSidebarTabs();

// File manager panel inside the FILES tab: host filesystem browsing plus the
// FluidNC controller and SD-card filesystems (open, upload, CRUD, transfers).
initFileManager(app);

// Repaint the scene and the dial when the viewport changes size. The dial
// check is lazy so this listener survives the handle-creation order below.
window.addEventListener("resize", () => {
  app.scene.draw();
  if (app.jogDial.isDialVisible()) app.jogDial.drawDial();
});

// Repaint every canvas whenever the theme changes. The 2D contexts read plain
// hex values (they cannot resolve CSS variables), so each renderer must run
// again after theme-colors.js has dropped its token cache.
window.addEventListener("cnc-theme-changed", () => {
  app.jogDial.drawDial();
  app.scene.draw();        // grid, stock, trajectory, axes and the orientation cube
  app.scene.drawTool();    // rotating tool on the overlay canvas
});

// Re-render dynamically built text whenever the locale changes at runtime.
// `applyTranslations` inside switchLocale() already updated every static
// data-i18n node; what is left are the pieces assembled in JS: the jog step
// dropdowns/buttons with translated tooltips and the compact zero/home
// captions, whose short-vs-full decision depends on the new text widths.
window.addEventListener("cnc-locale-changed", () => {
  app.jogDial.updateStepControls();
  compactLabels.refresh();
  // Rotary A/B/C captions, tooltips and ARIA labels carry the axis letter as a
  // parameter, so they are rebuilt in JS (jog-rotary-columns.js) rather than
  // through static data-i18n-* attributes.
  app.jogDial.rotaryColumns.refresh();
  // Axis rows (zero/home button titles and the clickable work-value tooltip)
  // are built with template strings, so re-apply their localized titles too.
  app.positionAxisView.refreshTitles();
  // Dial idle tooltip + hint line, and the hovered variant, are assembled in
  // JS as well; clearHover() rewrites both from the fresh dictionary.
  app.jogDial.modeControls.refreshDialChrome(app.jog.motionMode === "incremental");
  app.jogDial.interaction.clearHover();
  // Projection toggle tooltip/ARIA is written imperatively by the toolbar.
  app.scene.viewerToolbar.updateProjectionToggle();
  // The program name label ("NAME · N SEGMENTS") only updates on load, so
  // re-apply it here when a program is already displayed.
  app.program.refreshProgramName();
  // The state chip refreshes itself on the next status poll (translateState).
});

// Startup: freeze the controls until the first poll proves a live connection,
// then warm the console history, the default program and the status loop.
app.status.setJogEnabled(false);
commandConsole.loadHistory();
app.program.loader.loadDefault();
app.refreshStatus();
window.setInterval(() => app.refreshStatus(), 50);
window.setInterval(() => app.clock.render(), 250);
