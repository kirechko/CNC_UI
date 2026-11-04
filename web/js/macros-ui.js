/**
 * Macro panel UI: a compact list plus a large modal editor dialog.
 *
 * The sidebar panel (#macros-panel) renders one compact row per macro showing
 * only the name; descriptions use the shared styled tooltip bubble through a
 * data attribute so they do not create a second native title tooltip. A single button per row opens #macro-dialog — a modal
 * dialog modeled after the message-log dialog — where the G-code body
 * textarea takes the dominant share of the space and the remaining fields
 * (name, description, per-controller invocation mappings, save/delete
 * actions) sit in compact rows around it.
 */
import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";
import { createMacroButtonsUi, loadMacroButtons } from "./macro-buttons-ui.js";

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
  const tree = document.querySelector("#macro-tree");
  const contextMenu = document.querySelector("#macro-context-menu");
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
  const state = { macros: [], folders: [], query: "", editingId: null, busy: false, buttons: loadMacroButtons(), expandedFolders: new Set() };
  const { renderMacroButtons, addConfiguredMacroButton, submitMacroButtonPicker } = createMacroButtonsUi({
    state,
    macroButtons,
    macroButtonPicker,
    macroButtonPickerForm,
    macroButtonPickerSelect,
    addMacroButton,
    panelStatus,
    t,
  });

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

  /** Deletes one empty folder after asking for explicit confirmation. */
  async function deleteFolder(folder) {
    if (!folder || !window.confirm(t("macros.folders.deleteConfirm", { name: folder.name }))) return;
    try {
      await invokeOrFetch("delete_macro_folder", { folder_id: folder.id }, `/api/macro-folders/${encodeURIComponent(folder.id)}`, "DELETE");
      state.folders = state.folders.filter((item) => item.id !== folder.id);
      if (state.currentFolderId === folder.id) state.currentFolderId = folder.parentId ?? null;
      renderList();
    } catch (error) {
      panelStatus.textContent = error?.message || t("macros.folders.error.notEmpty");
    }
  }

  /** Creates a child folder, preserving the existing parent relationship API. */
  async function createFolder(parentId) {
    const name = window.prompt(t("macros.folders.prompt"));
    if (!name?.trim()) return;
    const folder = { id: createMacroId(), name: name.trim(), parentId: parentId ?? null };
    try {
      await invokeOrFetch("create_macro_folder", { folder }, "/api/macro-folders", "POST");
      state.folders.push(folder);
      if (parentId) state.expandedFolders.add(parentId);
      state.expandedFolders.add(folder.id);
      renderList();
      panelStatus.textContent = t("macros.folders.created");
    } catch (error) {
      panelStatus.textContent = error?.message || t("macros.error.saveFailed");
    }
  }

  /** Rebuilds the folder tree and the editor's nested-folder selector. */
  function renderFolderNavigation() {
    if (tree) {
      tree.replaceChildren();
      const childFolders = new Map();
      state.folders.forEach((folder) => {
        const parent = folder.parentId ?? "";
        childFolders.set(parent, [...(childFolders.get(parent) ?? []), folder]);
      });
      const macrosByFolder = new Map();
      state.macros.forEach((macro) => {
        const folder = macro.folderId ?? "";
        macrosByFolder.set(folder, [...(macrosByFolder.get(folder) ?? []), macro]);
      });
      const hasChildren = (folderId) => (childFolders.get(folderId) ?? []).length > 0 || (macrosByFolder.get(folderId) ?? []).length > 0;
      const buildChildren = (parentId, depth, parentNode) => {
        const folders = childFolders.get(parentId) ?? [];
        const macros = macrosByFolder.get(parentId) ?? [];
        for (const folder of folders) {
          const branch = document.createElement("div");
          branch.className = "macro-tree-branch";
          branch.setAttribute("role", "treeitem");
          branch.setAttribute("aria-expanded", String(state.expandedFolders.has(folder.id)));
          branch.dataset.folderId = folder.id;
          branch.style.setProperty("--tree-depth", depth);
          branch.addEventListener("contextmenu", (event) => showFolderMenu(event, folder));
          const line = document.createElement("div");
          line.className = "macro-tree-folder";
          const toggle = document.createElement("button");
          toggle.type = "button";
          toggle.className = "macro-tree-toggle";
          const expandable = hasChildren(folder.id);
          toggle.disabled = !expandable;
          toggle.setAttribute("aria-label", t(state.expandedFolders.has(folder.id) ? "macros.tree.collapse" : "macros.tree.expand"));
          toggle.textContent = expandable ? (state.expandedFolders.has(folder.id) ? "▾" : "▸") : "·";
          toggle.addEventListener("click", () => {
            if (!expandable) return;
            state.expandedFolders.has(folder.id) ? state.expandedFolders.delete(folder.id) : state.expandedFolders.add(folder.id);
            renderList();
          });
          const name = document.createElement("button");
          name.type = "button";
          name.className = "macro-tree-folder-name";
          name.textContent = `📁 ${folder.name}`;
          name.addEventListener("click", () => {
            state.expandedFolders.has(folder.id) ? state.expandedFolders.delete(folder.id) : state.expandedFolders.add(folder.id);
            renderList();
          });
          line.append(toggle, name);
          branch.append(line);
          if (state.expandedFolders.has(folder.id)) {
            const children = document.createElement("div");
            children.className = "macro-tree-children";
            children.setAttribute("role", "group");
            buildChildren(folder.id, depth + 1, children);
            branch.append(children);
          }
          parentNode.append(branch);
        }
        for (const macro of macros) parentNode.append(createMacroTreeItem(macro, depth));
      };
      buildChildren("", 0, tree);
    }
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

  /** Builds a compact macro leaf; double-click opens its editor and the arrow runs it. */
  function createMacroTreeItem(macro, depth) {
    const row = document.createElement("div");
    row.className = "macro-row macro-tree-macro";
    row.setAttribute("role", "treeitem");
    row.style.setProperty("--tree-depth", depth);
    row.dataset.macroId = macro.id;
    const name = textElement("span", "macro-row-name", macro.name);
    const description = String(macro.description ?? "")
      .replace("{rosetta.reactiveHint}", t("rosetta.reactiveHint"))
      .replace("Rosetta reactive macro:", t("rosetta.reactiveHint"));
    const hints = [description, ...(macro.tags ?? []).map((tag) => `#${tag}`)].filter(Boolean).join(" · ");
    name.dataset.tooltipTitle = hints;
    name.classList.add("macro-row-has-hint");
    name.setAttribute("aria-label", `${macro.name}. ${hints}`);
    row.addEventListener("dblclick", (event) => {
      if (!event.target.closest("button")) openEditor(macro);
    });
    const runButton = document.createElement("button");
    runButton.type = "button";
    runButton.className = "macro-row-run icon-button";
    runButton.dataset.tooltipTitle = t("macros.runTitle");
    runButton.setAttribute("aria-label", `${t("macros.runTitle")}: ${macro.name}`);
    runButton.innerHTML = '<svg aria-hidden="true" viewBox="0 0 16 16"><path d="M4 2.5v11L13 8z"/></svg>';
    runButton.addEventListener("click", () => window.dispatchEvent(new CustomEvent("cnc-macro-run", { detail: macro })));
    row.append(name, runButton);
    return row;
  }

  /** Opens the small file-manager context menu for one folder branch. */
  function showFolderMenu(event, folder) {
    if (!contextMenu) return;
    event.preventDefault();
    contextMenu.dataset.folderId = folder.id;
    contextMenu.hidden = false;
    contextMenu.style.left = `${Math.min(event.clientX, window.innerWidth - 190)}px`;
    contextMenu.style.top = `${Math.min(event.clientY, window.innerHeight - 100)}px`;
  }

  /** Renders the expanded folder tree or filtered macro search results. */
  function renderList() {
    list.replaceChildren();
    const query = state.query.trim().toLocaleLowerCase();
    const searching = query.length > 0;
    renderFolderNavigation();
    if (tree) tree.hidden = searching;
    list.hidden = !searching;
    if (addFolderButton) addFolderButton.disabled = searching;
    const visibleMacros = state.macros.filter((macro) => {
      if (!searching) return false;
      const haystack = [macro.name, macro.description, macro.body, ...(macro.tags ?? [])].join("\\n").toLocaleLowerCase();
      return haystack.includes(query);
    });
    count.textContent = t(searching ? "macros.searchCount" : "macros.count", { count: searching ? visibleMacros.length : state.macros.length });
    if (visibleMacros.length === 0 && searching) {
      list.append(textElement("p", "macro-empty", t("macros.searchEmpty")));
      return;
    }

    if (searching) visibleMacros.forEach((macro) => list.append(createMacroTreeItem(macro, 0)));
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
    list.replaceChildren();
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
      console.error("Failed to load macros and folders:", error);
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
  addFolderButton?.addEventListener("click", () => void createFolder(null));
  contextMenu?.addEventListener("click", (event) => {
    const action = event.target.closest("[data-macro-action]")?.dataset.macroAction;
    if (!action) return;
    const folder = state.folders.find((item) => item.id === contextMenu.dataset.folderId);
    contextMenu.hidden = true;
    if (action === "create-folder") void createFolder(folder?.id ?? null);
    else if (action === "delete-folder") void deleteFolder(folder);
  });
  document.addEventListener("click", (event) => {
    if (contextMenu && !contextMenu.contains(event.target)) contextMenu.hidden = true;
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && contextMenu) contextMenu.hidden = true;
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
