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
  const searchInput = document.querySelector("#macro-search");
  const panelStatus = document.querySelector("#macro-status");
  const addButton = document.querySelector("#macro-add");
  const addFolderButton = document.querySelector("#macro-folder-add");
  const deleteFolderButton = document.querySelector("#macro-folder-delete");
  const breadcrumb = document.querySelector("#macro-breadcrumb");
  const folderList = document.querySelector("#macro-folder-list");
  const macroButtons = document.querySelector("#macro-buttons-list");
  const macroButtonPicker = document.querySelector("#macro-button-picker");
  const macroButtonPickerForm = document.querySelector("#macro-button-picker-form");
  const macroButtonPickerSelect = document.querySelector("#macro-button-picker-select");
  const addMacroButton = document.querySelector("#macro-button-add");

  // The large modal editor dialog and every control it contains.
  const dialog = document.querySelector("#macro-dialog");
  const editor = document.querySelector("#macro-editor");
  const dialogTitle = document.querySelector("#macro-dialog-title");
  const nameInput = document.querySelector("#macro-name");
  const descriptionInput = document.querySelector("#macro-description");
  const bodyInput = document.querySelector("#macro-body");
  const tagsInput = document.querySelector("#macro-tags");
  const folderSelect = document.querySelector("#macro-folder-select");
  const invocationsBox = document.querySelector("#macro-invocations");
  const addInvocationButton = document.querySelector("#macro-invocation-add");
  const deleteButton = document.querySelector("#macro-delete");
  const cancelButton = document.querySelector("#macro-cancel");
  const closeDialogButton = document.querySelector("#macro-close");
  const errorBox = document.querySelector("#macro-error");
  const dialogStatus = document.querySelector("#macro-dialog-status");

  // The macro editor and navigation form the core panel; the shortcut UI remains optional.
  if (!panel || !list || !count || !panelStatus || !addButton || !dialog || !editor ||
      !dialogTitle || !nameInput || !descriptionInput || !bodyInput || !folderSelect || !invocationsBox ||
      !addInvocationButton || !deleteButton || !cancelButton || !closeDialogButton ||
      !errorBox || !dialogStatus) return;

  /** Holds macro data, navigation/filter state, editor state, and configured quick-run buttons. */
  const state = { macros: [], folders: [], currentFolderId: null, query: "", editingId: null, busy: false, buttons: loadMacroButtons() };

  /** Reads the user's macro-button assignments from local storage without failing private contexts. */
  function loadMacroButtons() {
    try {
      const value = JSON.parse(localStorage.getItem("cnc-ui-macro-buttons") || "[]");
      return Array.isArray(value) ? value.filter((id) => typeof id === "string") : [];
    } catch {
      return [];
    }
  }

  /** Persists button assignments locally; macro definitions themselves remain in the shared database. */
  function saveMacroButtons() {
    try {
      localStorage.setItem("cnc-ui-macro-buttons", JSON.stringify(state.buttons));
    } catch {
      panelStatus.textContent = t("macros.buttons.storageError");
    }
  }

  /** Renders one run button per configured macro and controls for adding/removing assignments. */
  function renderMacroButtons() {
    if (!macroButtons) return;
    macroButtons.replaceChildren();
    const previous = state.buttons.length;
    state.buttons = state.buttons.filter((id) => state.macros.some((macro) => macro.id === id));
    if (state.buttons.length !== previous) saveMacroButtons();
    state.buttons.forEach((id, index) => {
      const macro = state.macros.find((item) => item.id === id);
      if (!macro) return;
      const row = document.createElement("div");
      row.className = "macro-button-row";
      row.setAttribute("role", "listitem");
      const run = document.createElement("button");
      run.type = "button";
      run.className = "macro-button-run";
      run.setAttribute("aria-label", `${t("macros.runTitle")}: ${macro.name}`);
      run.textContent = macro.name;
      run.title = macro.description || macro.name;
      // The shortcut button and the standard list runner share the same event contract.
      run.addEventListener("click", () => window.dispatchEvent(new CustomEvent("cnc-macro-run", { detail: macro })));
      const remove = document.createElement("button");
      remove.type = "button";
      remove.className = "macro-button-remove";
      remove.textContent = "×";
      remove.title = t("macros.buttons.removeTitle");
      remove.setAttribute("aria-label", `${t("macros.buttons.removeTitle")}: ${macro.name}`);
      remove.addEventListener("click", () => {
        state.buttons.splice(index, 1);
        saveMacroButtons();
        renderMacroButtons();
      });
      row.append(run, remove);
      macroButtons.append(row);
    });
  }

  /** Creates and persists a button assignment for one macro selected by its id. */
  function assignMacroButton(macroId) {
    const macro = state.macros.find((item) => item.id === macroId);
    if (!macro || state.buttons.includes(macro.id)) return;
    state.buttons.push(macro.id);
    saveMacroButtons();
    renderMacroButtons();
  }

  /** Opens the in-page accessible chooser for selecting a macro shortcut. */
  function addConfiguredMacroButton() {
    const available = state.macros.filter((macro) => !state.buttons.includes(macro.id));
    if (!available.length) return;
    if (!macroButtonPicker || !macroButtonPickerForm || !macroButtonPickerSelect) {
      panelStatus.textContent = t("macros.buttons.pickerUnavailable");
      return;
    }
    macroButtonPickerSelect.replaceChildren();
    available.forEach((macro) => {
      const option = document.createElement("option");
      option.value = macro.id;
      option.textContent = macro.name;
      macroButtonPickerSelect.append(option);
    });
    macroButtonPicker.returnValue = "";
    macroButtonPicker.showModal();
    macroButtonPickerSelect.focus();
  }

  /** Submits the picker selection, leaving all assignments unchanged when the user cancels. */
  function submitMacroButtonPicker(event) {
    event.preventDefault();
    if (event.submitter?.value === "add") {
      assignMacroButton(macroButtonPickerSelect.value);
    }
    macroButtonPicker.close();
  }

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

  /** Returns the breadcrumb chain from root through the selected folder. */
  function folderChain(folderId) {
    const chain = [];
    let current = state.folders.find((folder) => folder.id === folderId);
    while (current) {
      chain.unshift(current);
      current = state.folders.find((folder) => folder.id === current.parentId);
    }
    return chain;
  }

  /** Renders directory navigation and the current folder's immediate children. */
  function renderFolderNavigation() {
    if (!folderList || !breadcrumb) return;
    folderList.replaceChildren();
    breadcrumb.replaceChildren();
    const addCrumb = (label, folderId) => {
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = label;
      button.addEventListener("click", () => { state.currentFolderId = folderId; renderList(); });
      breadcrumb.append(button);
    };
    addCrumb(t("macros.folders.root"), null);
    folderChain(state.currentFolderId).forEach((folder) => addCrumb(folder.name, folder.id));
    if (deleteFolderButton) {
      deleteFolderButton.hidden = !state.currentFolderId;
      deleteFolderButton.onclick = async () => {
        const folder = state.folders.find((item) => item.id === state.currentFolderId);
        if (!folder || !window.confirm(t("macros.folders.deleteConfirm", { name: folder.name }))) return;
        try {
          await invokeOrFetch("delete_macro_folder", { folder_id: folder.id }, `/api/macro-folders/${encodeURIComponent(folder.id)}`, "DELETE");
          state.folders = state.folders.filter((item) => item.id !== folder.id);
          state.currentFolderId = folder.parentId ?? null;
          renderList();
        } catch (error) { panelStatus.textContent = error?.message || t("macros.folders.error.notEmpty"); }
      };
    }
    state.folders.filter((folder) => (folder.parentId ?? null) === state.currentFolderId).forEach((folder) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "macro-folder-entry";
      button.textContent = `📁 ${folder.name}`;
      button.addEventListener("click", () => { state.currentFolderId = folder.id; renderList(); });
      folderList.append(button);
    });
    if (folderSelect) {
      const selected = folderSelect.value;
      folderSelect.replaceChildren(new Option(t("macros.folders.root"), ""));
      const children = new Map();
      state.folders.forEach((folder) => {
        const key = folder.parentId ?? "";
        children.set(key, [...(children.get(key) ?? []), folder]);
      });
      const appendTree = (parent, depth) => (children.get(parent) ?? []).forEach((folder) => {
        folderSelect.append(new Option(`${"　".repeat(depth)}${folder.name}`, folder.id));
        appendTree(folder.id, depth + 1);
      });
      appendTree("", 0);
      folderSelect.value = selected;
    }
  }

  /**
   * Renders the current directory or global search results, including macro hints
   * and the action buttons for editing and running each match.
   */
  function renderList() {
    list.replaceChildren();
    const query = state.query.trim().toLocaleLowerCase();
    const searching = query.length > 0;
    if (!searching) renderFolderNavigation();
    if (breadcrumb) breadcrumb.hidden = searching;
    if (folderList) folderList.hidden = searching;
    if (addFolderButton) addFolderButton.disabled = searching;
    if (deleteFolderButton) deleteFolderButton.disabled = searching;
    const childFolders = searching ? [] : state.folders.filter((folder) => (folder.parentId ?? null) === state.currentFolderId);
    const visibleMacros = state.macros.filter((macro) => {
      if (!searching && (macro.folderId ?? null) !== state.currentFolderId) return false;
      if (!searching) return true;
      const haystack = [macro.name, macro.description, macro.body, ...(macro.tags ?? [])].join("\\n").toLocaleLowerCase();
      return haystack.includes(query);
    });
    count.textContent = t(searching ? "macros.searchCount" : "macros.count", { count: visibleMacros.length });
    if (visibleMacros.length === 0 && childFolders.length === 0) {
      list.append(textElement("p", "macro-empty", t("macros.empty")));
      return;
    }

    visibleMacros.forEach((macro) => {
      const row = document.createElement("div");
      row.className = "macro-row";
      row.setAttribute("role", "listitem");

      const name = textElement("span", "macro-row-name", macro.name);
      // The description and tags stay available as hints without expanding the compact row.
      const hints = [macro.description, ...(macro.tags ?? []).map((tag) => `#${tag}`)].filter(Boolean).join(" · ");
      if (hints) {
        name.title = hints;
        row.title = hints;
      }

      // Rosetta runner entry (T26): ▶ opens the macro-run dialog through a
      // window event, so this CRUD module stays decoupled from the runner.
      const runButton = document.createElement("button");
      runButton.type = "button";
      runButton.className = "macro-row-run";
      runButton.textContent = "▶";
      runButton.title = t("macros.runTitle");
      runButton.setAttribute("aria-label", `${t("macros.runTitle")}: ${macro.name}`);
      runButton.addEventListener("click", () => {
        window.dispatchEvent(new CustomEvent("cnc-macro-run", { detail: macro }));
      });

      const openButton = document.createElement("button");
      openButton.type = "button";
      openButton.className = "macro-row-open";
      openButton.textContent = t("macros.edit");
      openButton.title = t("macros.openTitle");
      openButton.setAttribute("aria-label", `${t("macros.openTitle")}: ${macro.name}`);
      openButton.addEventListener("click", () => openEditor(macro));

      row.append(name, runButton, openButton);
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
    if (tagsInput) tagsInput.value = (macro?.tags ?? []).join(", ");
    renderFolderNavigation();
    folderSelect.value = macro?.folderId ?? state.currentFolderId ?? "";
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
      tags: (tagsInput?.value ?? "").split(",").map((tag) => tag.trim()).filter(Boolean),
      invocations,
      folderId: folderSelect.value || null,
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
      renderMacroButtons();
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
      const [result, folders] = await Promise.all([
        invokeOrFetch("get_macros", null, "/api/macros"),
        invokeOrFetch("get_macro_folders", null, "/api/macro-folders"),
      ]);
      state.macros = Array.isArray(result) ? result : [];
      state.folders = Array.isArray(folders) ? folders : [];
      if (state.currentFolderId && !state.folders.some((folder) => folder.id === state.currentFolderId)) state.currentFolderId = null;
      renderList();
      renderMacroButtons();
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
    renderMacroButtons();
    if (searchInput) searchInput.placeholder = t("macros.searchPlaceholder");
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

  searchInput?.addEventListener("input", () => {
    state.query = searchInput.value;
    renderList();
  });
  addButton.addEventListener("click", () => openEditor());
  addFolderButton?.addEventListener("click", async () => {
    const name = window.prompt(t("macros.folders.prompt"));
    if (!name?.trim()) return;
    const folder = { id: createMacroId(), name: name.trim(), parentId: state.currentFolderId };
    try {
      await invokeOrFetch("create_macro_folder", { folder }, "/api/macro-folders", "POST");
      state.folders.push(folder);
      renderList();
      panelStatus.textContent = t("macros.folders.created");
    } catch (error) {
      panelStatus.textContent = error?.message || t("macros.error.saveFailed");
    }
  });
  addMacroButton?.addEventListener("click", addConfiguredMacroButton);
  macroButtonPickerForm?.addEventListener("submit", submitMacroButtonPicker);
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
  renderMacroButtons();
  if (!panel.hidden) loadMacros();
}
