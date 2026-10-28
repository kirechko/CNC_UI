// Program panel controller: owns the currently loaded G-code program
// (name/content/backend segments), the toolpath presentation and the
// load/edit entry points. Extracted verbatim from main.js.
import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";
import { initProgramLoader } from "./program-loader.js";
import { initProgramEditor, setProgramEditorBaseline } from "./program-editor.js";
import { logMachineConsoleEvent, logMachineConsoleError } from "./machine-console-log.js";

/**
 * Creates the program panel.
 *
 * @param {object} app - Shared application bag exposing `clock` (toolpath
 *   time) and `scene` (camera + draw), which are assigned right after this
 *   module is created; all reads happen lazily inside handlers.
 * @returns {object} Panel handle: `getProgram`, `loader`,
 *   `refreshProgramName`.
 */
export function initProgramPanel(app) {
  const programName = document.querySelector("#program-name");
  let loadedProgram = null;

  /**
   * Loads the backend-parsed toolpath from `/api/toolpath` and shows the program.
   * The backend is the single source of toolpath segments.
   *
   * @param {string} name - Program file name.
   * @param {string} content - Program G-code text.
   */
  async function showProgram(name, content) {
    let toolpath = { segments: [], total_length_mm: 0 };
    try {
      toolpath = await invokeOrFetch("get_toolpath", null, "/api/toolpath");
    } catch (error) {
      document.querySelector("#message").textContent =
        `${t("program.error.trajectoryLoad")} ${error.message}`;
    }
    loadedProgram = { name, content, segments: toolpath.segments || [] };
    app.clock.update();
    setProgramEditorBaseline(content);
    document.querySelector("#job-note").textContent = t("main.programLoaded", {
      name,
      segments: loadedProgram.segments.length,
    });
    app.scene.camera.zoom = 1;
    app.scene.camera.panX = 0;
    app.scene.camera.panY = 0;
    if (programName) {
      programName.textContent = t("main.programNameSegments", {
        name,
        segments: loadedProgram.segments.length,
      });
    }
    app.scene.draw();
  }

  const programLoader = initProgramLoader({
    onLoad: showProgram,
    onError(error) {
      document.querySelector("#message").textContent = t("main.gcodeLoadError", {
        message: error.message,
      });
    },
  });

  initProgramEditor({
    getProgram: () => loadedProgram,
    logEvent: logMachineConsoleEvent,
    logError: logMachineConsoleError,
  });

  /**
   * Re-applies the localized "NAME · N SEGMENTS" program label after a
   * runtime locale switch; the label is only rebuilt on load otherwise.
   */
  function refreshProgramName() {
    if (loadedProgram && programName) {
      programName.textContent = t("main.programNameSegments", {
        name: loadedProgram.name,
        segments: loadedProgram.segments.length,
      });
    }
  }

  return {
    getProgram: () => loadedProgram,
    loader: programLoader,
    refreshProgramName,
    // Exposed for the file manager: after it pushes a file through the
    // backend `/api/program` endpoint it hands the parsed program here.
    showProgram,
  };
}
