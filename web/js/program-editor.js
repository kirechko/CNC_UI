import { invokeOrFetch } from "./api.js";

const EDITOR_ID = "#program-editor";

export function setProgramEditorBaseline(content) {
  const editor = document.querySelector(EDITOR_ID);
  if (!editor) return;

  editor.value = content;
  editor.dataset.originalContent = content;
}

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
      await logEvent(
        "предупреждение",
        "Невозможно загрузить программу из 3D-просмотра: программа не загружена.",
      );
      return;
    }

    setProgramEditorBaseline(program.content);
    await logEvent(
      "информация",
      `G-code загружен в редактор из 3D-просмотра: ${program.name}.`,
    );
  });

  saveButton?.addEventListener("click", async () => {
    const content = editor.value;
    if (!content.trim()) {
      await logEvent("ошибка", "Не удалось сохранить программу: редактор пуст.");
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
    await logEvent("информация", `G-code сохранён в файл: ${name}.`);
  });

  cancelButton?.addEventListener("click", async () => {
    editor.value = editor.dataset.originalContent || "";
    await logEvent(
      "информация",
      "Несохранённые изменения редактора G-code отменены.",
    );
  });

  dialog.addEventListener("cancel", () => {
    editor.value = editor.dataset.originalContent || "";
  });
}
