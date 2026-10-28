import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/** Maps job commands to their HTTP endpoint suffixes under `/api/program/`. */
const JOB_ENDPOINTS = {
  start_program: "run",
  pause_program: "pause",
  resume_program: "resume",
  abort_program: "abort",
};

/**
 * Sends a program job command to the backend.
 * @param {string} command One of the keys of `JOB_ENDPOINTS`.
 * @returns {Promise<void>} Resolves when the backend accepts the command.
 * @throws {Error} When the command is unknown or the request fails.
 */
async function sendJobCommand(command) {
  const endpoint = JOB_ENDPOINTS[command];
  if (!endpoint) {
    throw new Error(t("job.error.unknownCommand", { command }));
  }

  await invokeOrFetch(
    command,
    null,
    `/api/program/${endpoint}`,
    "POST",
  );
}

/**
 * Initializes the program run/pause/stop buttons and the job caption.
 * @param {{getProgram: Function, getMachineState: Function, refreshStatus: Function}} options
 * Accessors for the loaded program and machine state, and a status refresh callback.
 * @returns {{update: Function}} Object with `update(status)`, which refreshes button states and caption.
 */
export function initJobControls({ getProgram, getMachineState, refreshStatus }) {
  const runButton = document.querySelector("#job-run");
  const pauseButton = document.querySelector("#job-pause");
  const abortButton = document.querySelector("#job-abort");
  const caption = document.querySelector("#job-caption");
  const note = document.querySelector("#job-note");

  runButton.addEventListener("click", async () => {
    runButton.disabled = true;
    note.textContent = "";
    const paused = getMachineState() === "PAUSED";

    try {
      await sendJobCommand(paused ? "resume_program" : "start_program");
      note.textContent = paused
        ? t("job.note.resumeSent")
        : t("job.note.startSent");
      await refreshStatus();
    } catch (error) {
      note.textContent = t("job.error.control", { message: error.message });
    }
  });

  pauseButton.addEventListener("click", async () => {
    pauseButton.disabled = true;
    try {
      await sendJobCommand("pause_program");
      note.textContent = t("job.note.pauseSent");
      await refreshStatus();
    } catch (error) {
      note.textContent = t("job.error.pause", { message: error.message });
    }
  });

  abortButton.addEventListener("click", async () => {
    abortButton.disabled = true;
    try {
      await sendJobCommand("abort_program");
      note.textContent = t("job.note.stopSent");
      await refreshStatus();
    } catch (error) {
      note.textContent = t("job.error.stop", { message: error.message });
    }
  });

  return {
    /**
     * Updates button enablement, labels and the caption from the latest machine status.
     * @param {{connected?: boolean, state: string, current_line: number, total_lines: number}} status Latest status snapshot.
     * @returns {void}
     */
    update(status) {
      const hasProgram = Boolean(getProgram());
      const running = status.state === "READING";
      const paused = status.state === "PAUSED";
      const canControl = Boolean(status.connected) && hasProgram;
      const runLabel = paused ? t("job.run.resume") : t("job.run.start");

      runButton.disabled = !canControl || (running && !paused);
      runButton.title = runLabel;
      runButton.setAttribute("aria-label", runLabel);
      pauseButton.disabled = !canControl || !running;
      abortButton.disabled = !canControl || (!running && !paused);

      if (!hasProgram) {
        caption.textContent = t("job.caption.noProgram");
      } else if (running || paused) {
        const stateLabel = paused ? t("job.state.paused") : t("job.state.running");
        caption.textContent = t("job.caption.progress", {
          state: stateLabel,
          current: status.current_line,
          total: status.total_lines,
        });
      } else {
        caption.textContent = t("job.caption.loaded", { lines: status.total_lines });
      }
    },
  };
}
