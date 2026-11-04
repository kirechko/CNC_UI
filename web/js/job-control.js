import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/** Maps job commands to their HTTP endpoint suffixes under `/api/program/`. */
const JOB_ENDPOINTS = {
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
  const currentLine = document.querySelector("#job-current-line");
  const note = document.querySelector("#job-note");

  runButton.addEventListener("click", async () => {
    runButton.disabled = true;
    note.textContent = "";
    const paused = getMachineState() === "PAUSED";

    try {
      if (paused) {
        await sendJobCommand("resume_program");
        note.textContent = t("job.note.resumeSent");
      } else {
        const report = await invokeOrFetch("preflight_program", {}, "/api/program/preflight", "POST");
        renderPreflight(report);
        if (report.blocked) {
          renderPreflightDialog(report, false);
          return;
        }
        const warnings = report.findings.filter((finding) => finding.severity === "warning");
        const acknowledged = warnings.length === 0 || await renderPreflightDialog(report, true);
        renderPreflight(report);
        if (!acknowledged) {
          note.textContent = t("preflight.cancelled");
          return;
        }
        await invokeOrFetch("start_program_confirmed", {
          fingerprint: report.fingerprint,
          confirmedReview: true,
        }, "/api/program/run-confirmed", "POST");
        note.textContent = t("job.note.startSent");
      }
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

  /**
   * Localizes known preflight findings while retaining backend details for new or user-defined findings.
   * @param {object} finding Static analysis finding returned by the backend.
   * @returns {string} Localized description or the backend-provided fallback.
   */
  function localizePreflightFinding(finding) {
    const detail = finding.detail ?? "";
    const params = { detail, backend: "", axis: "", value: "", minimum: "", maximum: "", profile: "", tool: "", code: "" };
    const decodedCode = detail.match(/\b([GM]\d+(?:\.\d+)?)/)?.[1] ?? "";
    params.code = decodedCode;
    let key = `preflight.finding.${finding.code}`;
    let match = detail.match(/^(.+?) travel limit is not fully configured(?: in profile `([^`]+)`)?; this axis was not checked$/);
    if (match) {
      params.axis = match[1];
      params.profile = match[2] ? t("preflight.finding.profileContext", { profile: match[2] }) : "";
    } else if ((match = detail.match(/^([A-Z])=([-\d.]+) is outside effective configured range \[([-\d.]+), ([-\d.]+)\]$/))) {
      [, params.axis, params.value, params.minimum, params.maximum] = match;
    } else if ((match = detail.match(/^(.+?) capability for (.+?) is not verified by this static preflight/))) {
      params.backend = match[1];
      params.code = match[2];
    } else if ((match = detail.match(/^(.+?) cannot execute (.+?) natively$/))) {
      params.backend = match[1];
      params.code = match[2];
      key = "preflight.finding.capability_unsupported";
    } else if (detail === "no automatic tool changer") {
      key = "preflight.finding.no_automatic_tool_changer";
    } else if (detail === "rotation of coordinate system supported via rotate") {
      key = "preflight.finding.coordinate_rotation_partial";
    } else if ((match = detail.match(/^T(\d+) (?:matches a local tool record|has no local tool record)/))) {
      params.tool = match[1];
    } else if ((match = detail.match(/^travel check currently inspects arc endpoints, not the full swept arc envelope$/))) {
      key = "preflight.finding.arc_bounds_approximate";
    } else if ((match = detail.match(/^(.+?) exposes one coordinate offset; this program requests multiple G54-G59 systems$/))) {
      params.backend = match[1];
      key = "preflight.finding.unsupported_wcs";
    } else if ((match = detail.match(/^(.+?) probing, individual WCS support, and recovery semantics are not fully verified by this preflight$/))) {
      params.backend = match[1];
    } else if ((match = detail.match(/^(.+?) capability data is incomplete; static findings do not prove firmware execution support$/))) {
      params.backend = match[1];
    } else if ((match = detail.match(/^profile tool reference `([^`]+)`/))) {
      params.tool = match[1];
    } else if ((match = detail.match(/^(.+?) profile maximum travel ([\d.-]+) differs from controller maximum ([\d.-]+)$/))) {
      params.axis = match[1];
      params.value = match[2];
      params.maximum = match[3];
      key = "preflight.finding.profile_travel_conflict";
    } else if ((match = detail.match(/^(.+?) profile maximum speed ([\d.-]+) differs from controller maximum ([\d.-]+)$/))) {
      params.axis = match[1];
      params.value = match[2];
      params.maximum = match[3];
      key = "preflight.finding.profile_speed_conflict";
    } else if (finding.code === "profile_tool_reference_found" || finding.code === "profile_tool_reference_missing") {
      key = `preflight.finding.${finding.code}`;
    }
    const localized = t(key, params);
    if (localized !== key) return localized;
    return t("preflight.finding.unknown", { detail: detail || t("preflight.finding.noDetail") });
  }

  /** Shows a detailed, text-only preflight confirmation dialog and resolves with the operator's choice. */
  function renderPreflightDialog(report, canContinue) {
    const dialog = document.querySelector("#preflight-dialog");
    const summary = document.querySelector("#preflight-summary");
    const findingsList = document.querySelector("#preflight-findings");
    const continueButton = document.querySelector("#preflight-continue");
    const closeButton = document.querySelector("#preflight-close");
    const cancelButton = document.querySelector("#preflight-cancel");
    if (!dialog || !summary || !findingsList || !continueButton || !cancelButton) {
      const warnings = report.findings.filter((item) => item.severity === "warning");
      const details = warnings.map((item) => `${item.line ? t("preflight.line", { line: item.line }) + ": " : ""}${localizePreflightFinding(item)}`);
      const prompt = `${t("preflight.warningCount", { count: warnings.length, info: report.findings.filter((item) => item.severity === "info").length })}\\n\\n${details.join("\\n")}`;
      return Promise.resolve(canContinue ? window.confirm(prompt) : false);
    }
    const warningCount = report.findings.filter((item) => item.severity === "warning").length;
    const blockerCount = report.findings.filter((item) => item.severity === "blocker").length;
    const infoCount = report.findings.filter((item) => item.severity === "info").length;
    summary.textContent = canContinue
      ? t("preflight.warningCount", { count: warningCount, info: infoCount })
      : t("preflight.blockedSummary", { count: blockerCount });
    findingsList.replaceChildren();
    for (const finding of report.findings) {
      const item = document.createElement("li");
      item.className = `preflight-finding severity-${finding.severity}`;
      const heading = document.createElement("strong");
      heading.textContent = t(`preflight.severity.${finding.severity}`);
      const detail = document.createElement("span");
      detail.textContent = `${finding.line ? t("preflight.line", { line: finding.line }) + ": " : ""}${localizePreflightFinding(finding)}`;
      item.append(heading, detail);
      findingsList.append(item);
    }
    continueButton.hidden = !canContinue;
    let resolveChoice;
    const choice = new Promise((resolve) => { resolveChoice = resolve; });
    const finish = (accepted) => {
      if (dialog.open) dialog.close();
      resolveChoice(accepted);
    };
    continueButton.onclick = () => finish(true);
    cancelButton.onclick = () => finish(false);
    closeButton && (closeButton.onclick = () => finish(false));
    dialog.oncancel = (event) => { event.preventDefault(); finish(false); };
    dialog.showModal();
    return choice;
  }

  /** Renders current static findings into the visible job note without injecting HTML. */
  function renderPreflight(report) {
    const title = report.blocked ? t("preflight.note.blocked") : t("preflight.note.review");
    const findings = report.findings.map((finding) => {
      const location = finding.line ? `${t("preflight.line", { line: finding.line })}: ` : "";
      return `${t(`preflight.severity.${finding.severity}`)} — ${location}${localizePreflightFinding(finding)}`;
    });
    note.textContent = [title, ...findings].join("\n");
    window.dispatchEvent(new CustomEvent("cnc-preflight-report", { detail: report }));
  }

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
        if (currentLine) currentLine.hidden = true;
      } else if (running || paused) {
        const stateLabel = paused ? t("job.state.paused") : t("job.state.running");
        caption.textContent = t("job.caption.progress", {
          state: stateLabel,
          current: status.current_line,
          total: status.total_lines,
        });
        if (currentLine) {
          currentLine.hidden = false;
          currentLine.textContent = t("job.currentLine", { line: status.current_line });
        }
      } else {
        caption.textContent = t("job.caption.loaded", { lines: status.total_lines });
        if (currentLine) currentLine.hidden = true;
      }
    },
  };
}
