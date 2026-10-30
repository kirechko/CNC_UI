/**
 * Macro panel UI: a compact list plus a large modal editor dialog.
 *
 * The sidebar panel (#macros-panel) renders one compact row per macro showing
 * only the name; the description is surfaced as a tooltip hint (the shared
 * styled-tooltip engine in tooltips.js renders the `title` attribute as a
 * floating bubble). A single button per row opens #macro-dialog — a modal
 * dialog modeled after the message-log dialog — where the G-code body
 * textarea takes the dominant share of the space and the remaining fields
 * (name, description, per-controller invocation mappings, save/delete
 * actions) sit in compact rows around it.
 */
import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/**
 * Initializes macro loading, compact list rendering, the dialog editor, and
 * invocation-metadata editing.
 */
export function initMacrosUi() {
  // Compact list container and its surrounding panel chrome.
  const panel = document.querySelector("#macros-panel");
  const list = document.querySelector("#macro-list");
  const count = document.querySelector("#macros-count");
  const panelStatus = document.querySelector("#macro-status");
  const addButton = document.querySelector("#macro-add");

  // The large modal editor dialog and every control it contains.
  const dialog = document.querySelector("#macro-dialog");
  const editor = document.querySelector("#macro-editor");
  const dialogTitle = document.querySelector("#macro-dialog-title");
  const nameInput = document.querySelector("#macro-name");
  const descriptionInput = document.querySelector("#macro-description");
  const bodyInput = document.querySelector("#macro-body");
  const invocationsBox = document.querySelector("#macro-invocations");
  const addInvocationButton = document.querySelector("#macro-invocation-add");
  const deleteButton = document.querySelector("#macro-delete");
  const cancelButton = document.querySelector("#macro-cancel");
  const closeDialogButton = document.querySelector("#macro-close");
  const errorBox = document.querySelector("#macro-error");
  const dialogStatus = document.querySelector("#macro-dialog-status");

  // Abort early when the markup is absent (for example in a stripped Tauri build).
  if (!panel || !list || !count || !panelStatus || !addButton || !dialog || !editor ||
      !dialogTitle || !nameInput || !descriptionInput || !bodyInput || !invocationsBox ||
      !addInvocationButton || !deleteButton || !cancelButton || !closeDialogButton ||
      !errorBox || !dialogStatus) return;

  /** Holds the current server snapshot and the ID being edited, if any. */
  const state = { macros: [], editingId: null, busy: false };

  /** Creates a safe element with translated text and an optional CSS class. */
  function textElement(tag, className, text) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    element.textContent = text;
    return element;
  }

  /** Creates a new unique macro identifier using the platform UUID generator when available. */
  function createMacroId() {
    return globalThis.crypto?.randomUUID?.() ?? `macro-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
  }

  /**
   * Reports transient progress text in the visible surface: the dialog status
   * line while the modal is open, otherwise the panel status line.
   */
  function setStatus(message) {
    if (dialog.open) dialogStatus.textContent = message;
    else panelStatus.textContent = message;
  }

  /** Builds one editable controller invocation mapping row. */
  function createInvocationRow(invocation = {}) {
    const row = document.createElement("div");
    row.className = "macro-invocation-row";

    const controller = document.createElement("input");
    controller.type = "text";
    controller.maxLength = 80;
    controller.required = true;
    controller.placeholder = t("macros.controllerPlaceholder");
    controller.setAttribute("aria-label", t("macros.controller"));
    controller.value = invocation.controller ?? "";

    const mechanism = document.createElement("input");
    mechanism.type = "text";
    mechanism.maxLength = 80;
    mechanism.required = true;
    mechanism.placeholder = t("macros.mechanismPlaceholder");
    mechanism.setAttribute("aria-label", t("macros.mechanism"));
    mechanism.value = invocation.mechanism ?? "";

    const command = document.createElement("input");
    command.type = "text";
    command.maxLength = 128;
    command.required = true;
    command.placeholder = t("macros.commandPlaceholder");
    command.setAttribute("aria-label", t("macros.command"));
    command.value = invocation.command ?? "";

    const removeButton = document.createElement("button");
    removeButton.type = "button";
    removeButton.className = "macro-remove-invocation";
    removeButton.textContent = t("macros.removeInvocation");
    removeButton.addEventListener("click", () => row.remove());
    row.append(controller, mechanism, command, removeButton);
    return row;
  }

  /**
   * Renders the compact macro list: one row per macro showing only the name,
   * with the description exposed as a tooltip hint, plus a single button that
   * opens the large editor dialog for that macro.
   */
  function renderList() {
    list.replaceChildren();
    count.textContent = t("macros.count", { count: state.macros.length });
    if (state.macros.length === 0) {
      list.append(textElement("p", "macro-empty", t("macros.empty")));
      return;
    }

    state.macros.forEach((macro) => {
      const row = document.createElement("div");
      row.className = "macro-row";
      row.setAttribute("role", "listitem");

      const name = textElement("span", "macro-row-name", macro.name);
      // The description lives only in the tooltip hint to keep rows compact;
      // tooltips.js turns `title` into the styled floating bubble.
      if (macro.description) {
        name.title = macro.description;
        row.title = macro.description;
      }

      const openButton = document.createElement("button");
      openButton.type = "button";
      openButton.className = "macro-row-open";
      openButton.textContent = t("macros.edit");
      openButton.title = t("macros.openTitle");
      openButton.setAttribute("aria-label", `${t("macros.openTitle")}: ${macro.name}`);
      openButton.addEventListener("click", () => openEditor(macro));

      row.append(name, openButton);
      list.append(row);
    });
  }

  /** Opens the dialog editor for a new macro or populates it from an existing definition. */
  function openEditor(macro = null) {
    state.editingId = macro?.id ?? null;
    dialogTitle.textContent = t(macro ? "macros.editTitle" : "macros.addTitle");
    nameInput.value = macro?.name ?? "";
    descriptionInput.value = macro?.description ?? "";
    bodyInput.value = macro?.body ?? "";
    invocationsBox.replaceChildren();
    (macro?.invocations ?? []).forEach((invocation) => invocationsBox.append(createInvocationRow(invocation)));
    errorBox.hidden = true;
    errorBox.textContent = "";
    dialogStatus.textContent = "";
    // Deleting only makes sense for a macro that already exists on the server.
    deleteButton.hidden = state.editingId === null;
    dialog.showModal();
    nameInput.focus();
  }

  /** Closes the dialog and clears transient validation feedback. */
  function closeEditor() {
    if (dialog.open) dialog.close();
    errorBox.hidden = true;
    errorBox.textContent = "";
    dialogStatus.textContent = "";
    state.editingId = null;
  }

  /** Reads and validates the dialog fields into the API's macro data shape. */
  function readEditorMacro() {
    const invocations = [...invocationsBox.querySelectorAll(".macro-invocation-row")].map((row) => {
      const [controller, mechanism, command] = row.querySelectorAll("input");
      return {
        controller: controller.value.trim(),
        mechanism: mechanism.value.trim(),
        command: command.value.trim(),
      };
    });
    return {
      id: state.editingId ?? createMacroId(),
      name: nameInput.value.trim(),
      description: descriptionInput.value.trim(),
      body: bodyInput.value,
      invocations,
    };
  }

  /** Persists the complete macro set and updates the displayed snapshot after success. */
  async function persistMacros(nextMacros) {
    if (state.busy) return false;
    state.busy = true;
    setStatus(t("macros.saving"));
    try {
      await invokeOrFetch("put_macros", { macros: nextMacros }, "/api/macros", "PUT");
      state.macros = nextMacros;
      renderList();
      setStatus(t("macros.saved"));
      return true;
    } catch (error) {
      // Surface backend validation failures inside the dialog when it is open.
      if (dialog.open) {
        errorBox.textContent = error?.message || t("macros.error.saveFailed");
        errorBox.hidden = false;
        dialogStatus.textContent = "";
      } else {
        panelStatus.textContent = error?.message || t("macros.error.saveFailed");
      }
      return false;
    } finally {
      state.busy = false;
    }
  }

  /** Saves the dialog contents as a create or update operation, then closes the dialog. */
  async function saveEditor(event) {
    event.preventDefault();
    if (!editor.reportValidity()) return;
    const macro = readEditorMacro();
    if (!macro.name || !macro.body.trim()) {
      errorBox.textContent = t("macros.error.required");
      errorBox.hidden = false;
      return;
    }
    const existingIndex = state.macros.findIndex((entry) => entry.id === state.editingId);
    const nextMacros = [...state.macros];
    if (existingIndex < 0) nextMacros.push(macro);
    else nextMacros[existingIndex] = macro;
    if (await persistMacros(nextMacros)) closeEditor();
  }

  /** Removes the macro currently open in the dialog after an explicit confirmation. */
  async function removeCurrentMacro() {
    const macro = state.macros.find((entry) => entry.id === state.editingId);
    if (!macro || !window.confirm(t("macros.confirmRemove", { name: macro.name }))) return;
    if (await persistMacros(state.macros.filter((entry) => entry.id !== macro.id))) closeEditor();
  }

  /** Loads the saved macro collection and reports backend failures in the panel. */
  async function loadMacros() {
    if (state.busy) return;
    state.busy = true;
    setStatus(t("macros.loading"));
    try {
      const result = await invokeOrFetch("get_macros", null, "/api/macros");
      state.macros = Array.isArray(result) ? result : [];
      renderList();
      setStatus("");
    } catch (error) {
      list.replaceChildren(textElement("p", "macro-error", error?.message || t("macros.error.loadFailed")));
      setStatus("");
    } finally {
      state.busy = false;
    }
  }

  /** Reapplies translated labels to dynamically generated macro controls after a locale change. */
  function refreshLocalizedUi() {
    renderList();
    if (dialog.open) {
      dialogTitle.textContent = t(state.editingId ? "macros.editTitle" : "macros.addTitle");
      invocationsBox.querySelectorAll(".macro-invocation-row").forEach((row) => {
        const [controller, mechanism, command] = row.querySelectorAll("input");
        controller.placeholder = t("macros.controllerPlaceholder");
        controller.setAttribute("aria-label", t("macros.controller"));
        mechanism.placeholder = t("macros.mechanismPlaceholder");
        mechanism.setAttribute("aria-label", t("macros.mechanism"));
        command.placeholder = t("macros.commandPlaceholder");
        command.setAttribute("aria-label", t("macros.command"));
        row.querySelector("button").textContent = t("macros.removeInvocation");
      });
    }
  }

  addButton.addEventListener("click", () => openEditor());
  addInvocationButton.addEventListener("click", () => invocationsBox.append(createInvocationRow()));
  deleteButton.addEventListener("click", removeCurrentMacro);
  cancelButton.addEventListener("click", closeEditor);
  closeDialogButton.addEventListener("click", closeEditor);
  editor.addEventListener("submit", saveEditor);
  // ESC (and any other native dismissal) must reset transient editor state too.
  dialog.addEventListener("close", () => {
    errorBox.hidden = true;
    errorBox.textContent = "";
    state.editingId = null;
  });
  window.addEventListener("cnc-sidebar-tab-changed", (event) => {
    if (event.detail?.tab === "macros") loadMacros();
  });
  window.addEventListener("cnc-locale-changed", refreshLocalizedUi);
  renderList();
  if (!panel.hidden) loadMacros();
}
