import { t } from "./i18n.js";

const EDITOR_ID = "#program-editor";

// Optional external save target installed by the file manager when it opens a
// plain text file for editing. Shape: `{name, save(content) -> Promise}`. When
// present, the Save button writes back through the target instead of starting a
// browser download. Cleared automatically when the dialog closes.
let externalSaveTarget = null;

/**
 * Installs (or clears, with `null`) the external save target of the editor.
 * @param {{name: string, save: (content: string) => Promise<void>}|null} target
 */
export function setProgramEditorSaveTarget(target) {
  externalSaveTarget = target ?? null;
}

/**
 * Replaces the editor content and stores it as the saved baseline,
 * used later to discard unsaved changes.
 * @param {string} content - G-code text to show in the editor.
 */
export function setProgramEditorBaseline(content) {
  const editor = document.querySelector(EDITOR_ID);
  if (!editor) return;

  editor.value = content;
  editor.dataset.originalContent = content;
}

/**
 * Initializes the G-code editor dialog: load from the 3D view, save to file,
 * discard changes and close.
 * @param {{getProgram: Function, logEvent: Function, logError: Function}} deps
 */
export function initProgramEditor({ getProgram, logEvent, logError }) {
  const editor = document.querySelector(EDITOR_ID);
  const dialog = document.querySelector("#program-editor-dialog");
  const openButton = document.querySelector("#open-program-editor");
  const closeButton = document.querySelector("#close-program-editor");
  const loadButton = document.querySelector("#editor-load-program");
  const saveButton = document.querySelector("#editor-save-program");
  const cancelButton = document.querySelector("#editor-cancel-changes");

  if (!editor || !dialog) return;

  openButton?.addEventListener("click", () => {
    editor.dataset.originalContent = editor.value;
    dialog.showModal();
    editor.focus();
  });

  closeButton?.addEventListener("click", () => dialog.close());

  loadButton?.addEventListener("click", async () => {
    const program = getProgram();
    if (!program) {
      await logEvent("warning", t("program.editor.loadNoProgram"));
      return;
    }

    setProgramEditorBaseline(program.content);
    await logEvent("info", t("program.editor.loaded", { name: program.name }));
  });

  saveButton?.addEventListener("click", async () => {
    const content = editor.value;
    if (!content.trim()) {
      await logEvent("error", t("program.editor.emptySave"));
      return;
    }

    // File-manager mode: write the text back to the source it came from
    // instead of starting a browser download. Errors surface verbatim because
    // the backend already translates them for the active locale. The status
    // line `#message` gets an immediate copy of the note so the user sees the
    // result in the interface, not only in the backend message log (the
    // status poller may overwrite the node shortly afterwards; that matches
    // the existing transient-message behavior of the rest of the UI).
    if (externalSaveTarget) {
      const status = document.querySelector("#message");
      try {
        await externalSaveTarget.save(content);
        editor.dataset.originalContent = content;
        const note = t("program.editor.savedToSource", { name: externalSaveTarget.name });
        if (status) status.textContent = note;
        await logEvent("info", note);
      } catch (error) {
        const note = t("common.errorPrefix", { message: error.message });
        if (status) status.textContent = note;
        await logError("error", note);
      }
      return;
    }

    const name = getProgram()?.name || "program.ngc";
    const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = name;
    document.body.append(link);
    link.click();
    link.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 1000);

    editor.dataset.originalContent = content;
    await logEvent("info", t("program.editor.savedToFile", { name }));
  });

  cancelButton?.addEventListener("click", async () => {
    editor.value = editor.dataset.originalContent || "";
    await logEvent("info", t("program.editor.discarded"));
  });

  dialog.addEventListener("cancel", () => {
    editor.value = editor.dataset.originalContent || "";
  });

  // Leaving the dialog always drops the file-manager save-back hook, so the
  // next ordinary G-code edit keeps the default "save to download" behavior.
  dialog.addEventListener("close", () => {
    externalSaveTarget = null;
  });
}
