// Macro-list and editor UI for reusable G-code routines and controller-specific invocation metadata.
import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/**
 * Initializes macro loading, list rendering, CRUD controls, and invocation-metadata editing.
 */
export function initMacrosUi() {
  const panel = document.querySelector("#macros-panel");
  const list = document.querySelector("#macro-list");
  const count = document.querySelector("#macros-count");
  const editor = document.querySelector("#macro-editor");
  const editorTitle = document.querySelector("#macro-editor-title");
  const nameInput = document.querySelector("#macro-name");
  const descriptionInput = document.querySelector("#macro-description");
  const bodyInput = document.querySelector("#macro-body");
  const invocationsBox = document.querySelector("#macro-invocations");
  const errorBox = document.querySelector("#macro-error");
  const status = document.querySelector("#macro-status");
  const addButton = document.querySelector("#macro-add");
  const addInvocationButton = document.querySelector("#macro-invocation-add");
  const cancelButton = document.querySelector("#macro-cancel");

  if (!panel || !list || !count || !editor || !nameInput || !descriptionInput || !bodyInput ||
      !invocationsBox || !errorBox || !status || !addButton || !addInvocationButton || !cancelButton) return;

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

  /** Renders the macro collection with edit and delete actions. */
  function renderList() {
    list.replaceChildren();
    count.textContent = t("macros.count", { count: state.macros.length });
    if (state.macros.length === 0) {
      list.append(textElement("p", "macro-empty", t("macros.empty")));
      return;
    }

    state.macros.forEach((macro) => {
      const card = document.createElement("article");
      card.className = "macro-card";
      card.setAttribute("role", "listitem");
      const details = document.createElement("div");
      details.className = "macro-card-details";
      details.append(textElement("strong", "macro-card-name", macro.name));
      if (macro.description) details.append(textElement("span", "macro-card-description", macro.description));
      if (macro.invocations?.length) {
        const mappings = document.createElement("ul");
        mappings.className = "macro-card-invocations";
        macro.invocations.forEach((entry) => mappings.append(
          textElement("li", "", `${entry.controller} · ${entry.mechanism}: ${entry.command}`),
        ));
        details.append(mappings);
      }

      const actions = document.createElement("div");
      actions.className = "macro-card-actions";
      const editButton = document.createElement("button");
      editButton.type = "button";
      editButton.textContent = t("macros.edit");
      editButton.addEventListener("click", () => openEditor(macro));
      const removeButton = document.createElement("button");
      removeButton.type = "button";
      removeButton.className = "macro-delete-button";
      removeButton.textContent = t("macros.remove");
      removeButton.addEventListener("click", () => removeMacro(macro.id));
      actions.append(editButton, removeButton);
      card.append(details, actions);
      list.append(card);
    });
  }

  /** Opens the editor for a new macro or populates it from an existing definition. */
  function openEditor(macro = null) {
    state.editingId = macro?.id ?? null;
    editorTitle.textContent = t(macro ? "macros.editTitle" : "macros.addTitle");
    nameInput.value = macro?.name ?? "";
    descriptionInput.value = macro?.description ?? "";
    bodyInput.value = macro?.body ?? "";
    invocationsBox.replaceChildren();
    (macro?.invocations ?? []).forEach((invocation) => invocationsBox.append(createInvocationRow(invocation)));
    errorBox.hidden = true;
    errorBox.textContent = "";
    editor.hidden = false;
    nameInput.focus();
  }

  /** Closes the editor and clears transient validation feedback. */
  function closeEditor() {
    editor.hidden = true;
    errorBox.hidden = true;
    errorBox.textContent = "";
    state.editingId = null;
  }

  /** Reads and validates the editor fields into the API's macro data shape. */
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
    status.textContent = t("macros.saving");
    try {
      await invokeOrFetch("put_macros", { macros: nextMacros }, "/api/macros", "PUT");
      state.macros = nextMacros;
      renderList();
      status.textContent = t("macros.saved");
      return true;
    } catch (error) {
      errorBox.textContent = error?.message || t("macros.error.saveFailed");
      errorBox.hidden = false;
      status.textContent = "";
      return false;
    } finally {
      state.busy = false;
    }
  }

  /** Saves the current editor as a create or update operation. */
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

  /** Removes one macro after an explicit confirmation and persists the remaining collection. */
  async function removeMacro(id) {
    const macro = state.macros.find((entry) => entry.id === id);
    if (!macro || !window.confirm(t("macros.confirmRemove", { name: macro.name }))) return;
    await persistMacros(state.macros.filter((entry) => entry.id !== id));
  }

  /** Loads the saved macro collection and reports backend failures in the panel. */
  async function loadMacros() {
    if (state.busy) return;
    state.busy = true;
    status.textContent = t("macros.loading");
    try {
      const result = await invokeOrFetch("get_macros", null, "/api/macros");
      state.macros = Array.isArray(result) ? result : [];
      renderList();
      status.textContent = "";
    } catch (error) {
      list.replaceChildren(textElement("p", "macro-error", error?.message || t("macros.error.loadFailed")));
      status.textContent = "";
    } finally {
      state.busy = false;
    }
  }

  /** Reapplies translated labels to dynamically generated macro controls after a locale change. */
  function refreshLocalizedUi() {
    renderList();
    if (!editor.hidden) {
      editorTitle.textContent = t(state.editingId ? "macros.editTitle" : "macros.addTitle");
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
  cancelButton.addEventListener("click", closeEditor);
  editor.addEventListener("submit", saveEditor);
  window.addEventListener("cnc-sidebar-tab-changed", (event) => {
    if (event.detail?.tab === "macros") loadMacros();
  });
  window.addEventListener("cnc-locale-changed", refreshLocalizedUi);
  renderList();
  if (!panel.hidden) loadMacros();
}
