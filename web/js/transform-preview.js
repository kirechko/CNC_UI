/**
 * Rosetta transform preview (T26 §3.3/§3.4): validates the loaded program
 * against the connected controller and, when automatic resolutions exist,
 * shows the «was / became» diff before committing anything.
 *
 * Backend contract (see `src/web_server/handlers/rosetta.rs`):
 *   POST /api/program/validate          -> ValidationReport
 *     {backend, clean, findings:[{line,code,severity,resolvable,detail}]}
 *   POST /api/program/transform/preview -> PreviewReport
 *     {backend, plan, unresolved:[String],
 *      artifact:{source_text, emitted_text, traces:[{name,note,affected_lines}]}}
 *   POST /api/program/transform/apply   {plan, approved} -> Artifact
 *
 * Wire note: `rosetta` types keep their snake_case field names (the crate is
 * serialization-neutral), while the app models are camelCase — the UI reads
 * snake_case for the rosetta payloads exactly as emitted.
 *
 * Flow:
 *   Validate button → clean report is announced inline; findings open the
 *   preview dialog → Apply re-runs the *gated* pipeline on the backend and
 *   reloads the program into the viewer via `program.showProgram`.
 */
import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/**
 * Initializes the validate toolbar button and the transform preview dialog.
 * @param {object} options - Lazy handles from the application bag.
 * @param {Function} options.getProgram - Returns {name, content} | null.
 * @param {Function} options.showProgram - Reloads name/content into the viewer.
 */
export function initTransformPreview({ getProgram, showProgram }) {
  const validateButton = document.querySelector("#validate-program");
  const dialog = document.querySelector("#transform-dialog");
  const summary = document.querySelector("#transform-summary");
  const sourceBox = document.querySelector("#transform-source");
  const outputBox = document.querySelector("#transform-output");
  const tracesBox = document.querySelector("#transform-traces");
  const applyButton = document.querySelector("#transform-apply");
  const cancelButton = document.querySelector("#transform-cancel");
  const closeButton = document.querySelector("#transform-close");
  const stateLabel = document.querySelector("#transform-state");
  const errorBox = document.querySelector("#transform-error");

  // Stripped builds without the dialog markup degrade to validation-only.
  const hasDialog = dialog && summary && sourceBox && outputBox && tracesBox &&
    applyButton && cancelButton && closeButton && stateLabel && errorBox;

  /** The plan currently on display; Apply POSTs it back verbatim. */
  let current = null;

  /** One `pre` line highlight class from the plan trace (affected lines). */
  function renderCode(box, text, affected) {
    box.replaceChildren();
    const affectedSet = new Set(affected || []);
    text.split("\n").forEach((line, index) => {
      const span = document.createElement("span");
      span.className = affectedSet.has(index + 1) ? "transform-changed" : "";
      span.textContent = line + "\n";
      box.append(span);
    });
  }

  /** Shows a transient message in the dialog status line. */
  function showError(message) {
    if (!hasDialog) {
      document.querySelector("#message").textContent = message;
      return;
    }
    errorBox.textContent = message;
    errorBox.hidden = false;
  }

  /** Fills the two-column diff from the preview artifact. */
  function openPreview(report) {
    current = report;
    errorBox.hidden = true;
    stateLabel.textContent = "";
    summary.textContent = t("transform.summary", {
      backend: report.backend,
      steps: report.plan.steps.length,
      findings: report.artifact.findings.length,
    });
    const affected = report.artifact.traces.flatMap((trace) => trace.affected_lines || []);
    renderCode(sourceBox, report.artifact.source_text, affected);
    renderCode(outputBox, report.artifact.emitted_text, affected);
    tracesBox.replaceChildren();
    report.artifact.traces.forEach((trace) => {
      const item = document.createElement("li");
      const lines = (trace.affected_lines || []).join(", ");
      item.textContent = `${trace.name}` +
        (lines ? ` (${t("transform.lines")}: ${lines})` : "") +
        (trace.note ? ` — ${trace.note}` : "");
      tracesBox.append(item);
    });
    (report.unresolved || []).forEach((prompt) => {
      const item = document.createElement("li");
      item.className = "transform-unresolved";
      item.textContent = prompt;
      tracesBox.append(item);
    });
    applyButton.disabled = report.plan.steps.length === 0;
    dialog.showModal();
  }

  validateButton?.addEventListener("click", async () => {
    const program = getProgram();
    if (!program) {
      showError(t("transform.error.noProgram"));
      return;
    }
    validateButton.disabled = true;
    try {
      const report = await invokeOrFetch(
        "validate_program",
        { content: program.content },
        "/api/program/validate",
        "POST",
      );
      if (report.clean) {
        document.querySelector("#job-note").textContent =
          t("transform.clean", { backend: report.backend });
        return;
      }
      const list = report.findings
        .map((finding) => `${finding.code} @${finding.line}: ${finding.detail}`)
        .join("\n");
      // Findings without any resolvable fix cannot be previewed meaningfully.
      const anythingResolvable = report.findings.some((finding) => finding.resolvable);
      if (!anythingResolvable || !hasDialog) {
        document.querySelector("#job-note").textContent =
          `${t("transform.blocked", { backend: report.backend })}\n${list}`;
        return;
      }
      const preview = await invokeOrFetch(
        "transform_preview",
        { content: program.content },
        "/api/program/transform/preview",
        "POST",
      );
      if (preview.plan.steps.length === 0) {
        document.querySelector("#job-note").textContent =
          `${t("transform.unsupported", { backend: report.backend })}\n${list}`;
        return;
      }
      openPreview(preview);
    } catch (error) {
      showError(error.message);
    } finally {
      validateButton.disabled = false;
    }
  });

  if (hasDialog) {
    applyButton.addEventListener("click", async () => {
      if (!current) return;
      applyButton.disabled = true;
      stateLabel.textContent = t("transform.applying");
      errorBox.hidden = true;
      try {
        const artifact = await invokeOrFetch(
          "transform_apply",
          { plan: current.plan, approved: true },
          "/api/program/transform/apply",
          "POST",
        );
        // The backend already reloaded the controller program; refresh the
        // viewer/editor buffer with the emitted text through the panel API.
        const name = current.artifact?.plan_id ?? "transformed";
        const program = getProgram();
        showProgram(`${program?.name ?? name} (${t("transform.suffix")})`, artifact.emitted_text);
        stateLabel.textContent = t("transform.applied");
        setTimeout(() => dialog.close(), 600);
      } catch (error) {
        applyButton.disabled = false;
        showError(error.message);
      }
    });
    cancelButton.addEventListener("click", () => dialog.close());
    closeButton.addEventListener("click", () => dialog.close());
    dialog.addEventListener("close", () => {
      current = null;
    });
  }
}
