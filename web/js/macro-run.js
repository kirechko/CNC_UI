/**
 * Rosetta macro runner (T26): a modal that starts a macro run, tails the
 * engine log and can cancel the run cooperatively.
 *
 * Opened from the ▶ button of every macro row in the MACROS panel: the row
 * dispatches a `cnc-macro-run` window event carrying the macro object, this
 * module listens for it and opens the dialog — keeping the CRUD panel and the
 * runner independent.
 *
 * Backend contract (see `src/web_server/handlers/rosetta.rs`):
 *   POST /api/macros/run            {id, dry, args}  -> {run_id} (HTTP) or
 *                                                      run_id string (Tauri)
 *   GET  /api/macros/run/:run_id?from=N              -> RunStatus JSON
 *   POST /api/macros/run/:run_id/cancel              -> 204
 *
 * The log is polled with a growing `from` offset once per second while the
 * run is active; terminal states (`completed/aborted/cancelled/failed`) stop
 * the polling and report the result line.
 */
import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/** Poll interval of the run log tail, milliseconds. */
const POLL_MS = 1000;

/**
 * Initializes the macro-run dialog and its event wiring.
 * @returns {{open: (macro: object) => void}} handle for programmatic opening.
 */
export function initMacroRun() {
  const dialog = document.querySelector("#macro-run-dialog");
  const title = document.querySelector("#macro-run-title");
  const macroLabel = document.querySelector("#macro-run-macro");
  const dryCheckbox = document.querySelector("#macro-run-dry");
  const startButton = document.querySelector("#macro-run-start");
  const stopButton = document.querySelector("#macro-run-cancel");
  const stateLabel = document.querySelector("#macro-run-state");
  const logBox = document.querySelector("#macro-run-log");
  const errorBox = document.querySelector("#macro-run-error");
  const closeButton = document.querySelector("#macro-run-close");

  // Graceful degradation: a stripped Tauri build may lack the dialog markup.
  if (!dialog || !title || !macroLabel || !dryCheckbox || !startButton || !stopButton ||
      !stateLabel || !logBox || !errorBox || !closeButton) {
    return { open() {} };
  }

  /** Current run bookkeeping; null while no run is active or shown. */
  const run = { id: null, timer: 0, offset: 0, macro: null };

  /** Appends one engine log line to the tailing box (auto-scrolls). */
  function appendLine(line) {
    const row = document.createElement("div");
    row.className = `macro-run-line ${line.level}`;
    const step = line.step ? `[${line.step}] ` : "";
    row.textContent = `${step}${line.message}`;
    logBox.append(row);
    logBox.scrollTop = logBox.scrollHeight;
  }

  /** Renders the terminal state chip and re-enables the start button. */
  function finish(status) {
    startButton.disabled = false;
    stopButton.disabled = true;
    stateLabel.textContent = `${t(`macros.run.state.${status.state}`)}` +
      (status.result ? ` — ${status.result}` : "");
  }

  /** Pulls the next log chunk and keeps polling while the run is active. */
  async function poll() {
    if (!run.id) return;
    try {
      // HTTP: offset travels in the query string; `args` must stay null
      // because fetch rejects a body on GET requests. The Tauri branch still
      // receives the arguments object for the desktop command.
      const status = await invokeOrFetch(
        "get_macro_run_status",
        window.__TAURI__?.core?.invoke
          ? { run_id: run.id, from: run.offset }
          : null,
        `/api/macros/run/${encodeURIComponent(run.id)}?from=${run.offset}`,
      );
      (status.lines || []).forEach(appendLine);
      run.offset = status.next_offset;
      if (status.state !== "running") {
        clearInterval(run.timer);
        run.timer = 0;
        finish(status);
        return;
      }
      stateLabel.textContent = t(`macros.run.state.${status.state}`);
    } catch (error) {
      clearInterval(run.timer);
      run.timer = 0;
      showError(error.message);
    }
  }

  /** Shows a transient error inside the dialog. */
  function showError(message) {
    errorBox.textContent = message;
    errorBox.hidden = false;
  }

  /** Clears error and log state for a fresh dialog session. */
  function resetView() {
    logBox.replaceChildren();
    errorBox.hidden = true;
    errorBox.textContent = "";
    stateLabel.textContent = "";
    run.offset = 0;
  }

  /** Opens the dialog for one macro (dry-run pre-checked, ТЗ №2 §9.4). */
  function open(macro) {
    run.macro = macro;
    macroLabel.textContent = macro.name || macro.id;
    dryCheckbox.checked = true;
    resetView();
    startButton.disabled = false;
    stopButton.disabled = true;
    dialog.showModal();
  }

  /** Stops the local polling only; the machine run keeps going until cancel. */
  function stopPolling() {
    if (run.timer) {
      clearInterval(run.timer);
      run.timer = 0;
    }
  }

  startButton.addEventListener("click", async () => {
    if (!run.macro) return;
    startButton.disabled = true;
    errorBox.hidden = true;
    resetView();
    try {
      const result = await invokeOrFetch(
        "run_macro",
        { id: run.macro.id, dry: dryCheckbox.checked, args: [] },
        "/api/macros/run",
        "POST",
      );
      // HTTP answers {run_id}; the Tauri command returns the bare string.
      run.id = typeof result === "string" ? result : result?.run_id;
      if (!run.id) throw new Error(t("macros.run.error.noId"));
      stopButton.disabled = false;
      stateLabel.textContent = t("macros.run.state.running");
      stopPolling();
      run.timer = setInterval(poll, POLL_MS);
      poll();
    } catch (error) {
      startButton.disabled = false;
      showError(error.message);
    }
  });

  stopButton.addEventListener("click", async () => {
    if (!run.id) return;
    stopButton.disabled = true;
    try {
      await invokeOrFetch(
        "cancel_macro_run",
        { run_id: run.id },
        `/api/macros/run/${encodeURIComponent(run.id)}/cancel`,
        "POST",
      );
      stateLabel.textContent = t("macros.run.cancelling");
    } catch (error) {
      stopButton.disabled = false;
      showError(error.message);
    }
  });

  closeButton.addEventListener("click", () => dialog.close());
  dialog.addEventListener("close", () => {
    // Closing the dialog detaches the tail; an active run keeps executing on
    // the backend (cancel it with Stop before closing to abort).
    stopPolling();
  });

  // The MACROS panel rows dispatch this event with their macro object.
  window.addEventListener("cnc-macro-run", (event) => {
    if (event.detail?.id) open(event.detail);
  });

  return { open };
}
