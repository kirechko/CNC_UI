// File-manager panel UI: rendered lazily inside `#sidebar-content` (right
// after the legacy `.sidebar-empty` placeholder) and shown only while the
// FILES tab is active, so the MACROS tab keeps the original look. Provides
// source chips (host / FluidNC flash / SD card), a path bar, folder listings
// and per-entry actions: open, download, copy/cut+paste, rename, delete, plus
// new-folder and upload controls.
//
// "Open" is dispatched by file kind (see file-manager.js):
//   - G-code  -> backend `load_program` + program panel (toolpath in 3D view);
//   - 3D mesh -> stock parsers + `app.scene.applyStockMesh` (stock in 3D view);
//   - text    -> the shared G-code editor dialog with a save-back target;
//   - other   -> plain browser download.
// All backend errors arrive translated for the active locale, so the panel
// shows `error.message` verbatim inside the common error prefix.
import { t } from "./i18n.js";
import { invokeOrFetch } from "./api.js";
import { parseObj } from "./obj.js";
import { finalizeStockMesh, parseStl, parseThreeJsJson } from "./stock.js";
import { setProgramEditorBaseline, setProgramEditorSaveTarget } from "./program-editor.js";
import { dirOf, formatBytes, formatDate, iconButton, notify, notifyError, svg } from "./file-manager-widgets.js";
import {
  copyEntry,
  deleteEntry,
  downloadUrl,
  fetchBytes,
  fetchContent,
  fetchList,
  fetchSources,
  fileKind,
  makeFolder,
  moveEntry,
  renameEntry,
  uploadFile,
} from "./file-manager.js";

/**
 * Mounts the file-manager panel. Called once from main.js after the program
 * panel and scene stage exist in the shared `app` bag; every cross-feature
 * call happens lazily inside handlers, so creation order beyond that is safe.
 * @param {object} app Shared application bag (`app.program`, `app.scene`).
 */
export function initFileManager(app) {
  const sidebar = document.querySelector("#sidebar-content");
  if (!sidebar || document.querySelector("#file-manager")) return;

  // Panel state: selected source + directory, the last listing, the pending
  // clipboard entry and a re-entrancy guard for all async operations.
  const state = {
    sources: [],
    source: "local",
    path: "/",
    listing: null,
    clipboard: null,
    busy: false,
  };

  const root = document.createElement("div");
  root.className = "file-manager";
  root.id = "file-manager";

  const chipsRow = document.createElement("div");
  chipsRow.className = "fm-sources";
  chipsRow.setAttribute("role", "group");
  chipsRow.setAttribute("aria-label", t("files.ui.sourcesAria"));

  const pathBar = document.createElement("div");
  pathBar.className = "fm-path-bar";
  const upButton = iconButton("up", "files.ui.up", () => goTo(dirOf(state.path)));
  const pathLabel = document.createElement("span");
  pathLabel.className = "fm-path";
  pathLabel.textContent = "/";
  const refreshButton = iconButton("refresh", "files.ui.refresh", () => refresh());
  pathBar.append(upButton, pathLabel, refreshButton);

  const usageLine = document.createElement("div");
  usageLine.className = "fm-usage";
  usageLine.hidden = true;

  const actionRow = document.createElement("div");
  actionRow.className = "fm-actions-bar";
  const pasteButton = iconButton("paste", "files.ui.paste", () => pasteClipboard(), true);
  actionRow.append(
    iconButton("newFolder", "files.ui.newFolder", () => promptNewFolder()),
    iconButton("upload", "files.ui.upload", () => pickAndUpload()),
    pasteButton,
  );

  const listBox = document.createElement("div");
  listBox.className = "fm-list";
  listBox.setAttribute("aria-live", "polite");

  root.append(chipsRow, pathBar, usageLine, actionRow, listBox);
  sidebar.appendChild(root);

  /** Replaces the source chips; disabled chips cannot be selected. */
  function renderChips() {
    chipsRow.textContent = "";
    chipsRow.setAttribute("aria-label", t("files.ui.sourcesAria"));
    state.sources.forEach((source) => {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "fm-chip";
      chip.textContent = source.label;
      // Tooltip + accessible name: disabled chips still explain what they are
      // (and the hint-coverage audit expects a title on every control).
      chip.title = source.label;
      chip.setAttribute("aria-label", source.label);
      chip.disabled = !source.available;
      chip.classList.toggle("active", source.id === state.source);
      if (source.id !== state.source) {
        chip.addEventListener("click", () => {
          state.source = source.id;
          state.path = "/";
          refresh();
        });
      }
      chipsRow.appendChild(chip);
    });
  }

  /** Renders one directory row: name button (opens/navigates) + actions. */
  function renderEntry(entry) {
    const row = document.createElement("div");
    row.className = "fm-entry";
    row.dataset.fmEntry = entry.path;

    const icon = document.createElement("span");
    icon.className = "fm-entry-icon";
    icon.innerHTML = svg(entry.isDir ? "folder" : "file");

    const nameButton = document.createElement("button");
    nameButton.type = "button";
    nameButton.className = "fm-entry-name";
    nameButton.textContent = entry.name;
    nameButton.title = entry.path;
    nameButton.addEventListener("click", () => openEntry(entry));

    const meta = document.createElement("span");
    meta.className = "fm-entry-meta";
    const parts = [entry.isDir ? "" : formatBytes(entry.size)];
    if (entry.modifiedMs) parts.push(formatDate(entry.modifiedMs));
    meta.textContent = parts.filter(Boolean).join(" · ");

    const actions = document.createElement("span");
    actions.className = "fm-entry-actions";
    actions.append(
      iconButton("open", "files.ui.open", () => openEntry(entry)),
      ...(entry.isDir
        ? []
        : [iconButton("download", "files.ui.download", () => startDownload(entry))]),
      iconButton("copy", "files.ui.copyFile", () => setClipboard(entry, "copy")),
      iconButton("cut", "files.ui.cutFile", () => setClipboard(entry, "move")),
      iconButton("rename", "files.ui.rename", () => promptRename(entry)),
      iconButton("remove", "files.ui.remove", () => confirmDelete(entry)),
    );

    row.append(icon, nameButton, meta, actions);
    return row;
  }

  /** Rebuilds the entry list, the path label and the volume usage line. */
  function renderListing() {
    pathLabel.textContent = state.path;
    upButton.disabled = state.path === "/";
    listBox.textContent = "";
    const listing = state.listing;
    if (!listing) {
      appendNote(listBox, t("files.ui.loading"));
      usageLine.hidden = true;
      return;
    }
    usageLine.hidden = listing.totalBytes == null;
    if (!usageLine.hidden) {
      usageLine.textContent = t("files.ui.usage", {
        used: formatBytes(listing.usedBytes ?? 0),
        total: formatBytes(listing.totalBytes ?? 0),
        percent: String(listing.occupationPercent ?? 0),
      });
    }
    if (!listing.entries.length) {
      appendNote(listBox, t("files.ui.empty"));
      return;
    }
    listing.entries.forEach((entry) => listBox.appendChild(renderEntry(entry)));
  }

  /** Adds a single muted note line to the list area. */
  function appendNote(container, text) {
    const note = document.createElement("div");
    note.className = "fm-note";
    note.textContent = text;
    container.appendChild(note);
  }

  /** Fetches sources + current directory and re-renders the whole panel. */
  async function refresh() {
    if (state.busy) return;
    state.busy = true;
    renderChips();
    renderListing();
    try {
      state.sources = await fetchSources();
      if (!state.sources.some((s) => s.id === state.source && s.available)) {
        const first = state.sources.find((s) => s.available);
        if (first) {
          state.source = first.id;
          state.path = "/";
        }
      }
      state.listing = await fetchList(state.source, state.path);
      state.path = state.listing.path;
    } catch (error) {
      state.listing = { path: state.path, entries: [] };
      notifyError(error);
    } finally {
      state.busy = false;
      renderChips();
      renderListing();
      pasteButton.disabled = !state.clipboard;
    }
  }

  /** Navigates into `path` (always used with a directory path). */
  function goTo(path) {
    state.path = path;
    state.listing = null;
    refresh();
  }

  /** Asks for a folder name and creates it inside the current directory. */
  async function promptNewFolder() {
    const name = window.prompt(t("files.ui.promptFolder"), "");
    if (!name || state.busy) return;
    try {
      await makeFolder(state.source, state.path, name.trim());
      notify(t("files.ui.folderCreated", { name }));
      await refresh();
    } catch (error) {
      notifyError(error);
    }
  }

  /** Opens the native picker and uploads the chosen file into the current directory. */
  function pickAndUpload() {
    const input = document.createElement("input");
    input.type = "file";
    input.hidden = true;
    document.body.appendChild(input);
    input.addEventListener("change", async () => {
      const file = input.files?.[0];
      input.remove();
      if (!file) return;
      try {
        await uploadFile(state.source, state.path, file.name, file);
        notify(t("files.ui.uploaded", { name: file.name }));
        await refresh();
      } catch (error) {
        notifyError(error);
      }
    });
    input.click();
  }

  /** Starts a browser download of the file through `/api/files/download`. */
  function startDownload(entry) {
    const link = document.createElement("a");
    link.href = downloadUrl(state.source, entry.path);
    link.download = entry.name;
    document.body.appendChild(link);
    link.click();
    link.remove();
  }

  /** Stores the clipboard entry and enables the paste button. */
  function setClipboard(entry, mode) {
    state.clipboard = { source: state.source, path: entry.path, name: entry.name, mode };
    pasteButton.disabled = false;
    notify(t(mode === "move" ? "files.ui.cutHint" : "files.ui.copiedHint", { name: entry.name }));
  }

  /** Applies the stored clipboard entry to the current directory. */
  async function pasteClipboard() {
    const clip = state.clipboard;
    if (!clip || state.busy) return;
    try {
      if (clip.mode === "move") {
        await moveEntry(clip.source, clip.path, state.source, state.path);
        notify(t("files.ui.moved", { name: clip.name }));
      } else {
        await copyEntry(clip.source, clip.path, state.source, state.path);
        notify(t("files.ui.copied", { name: clip.name }));
      }
      state.clipboard = null;
      await refresh();
    } catch (error) {
      notifyError(error);
    }
  }

  /** Asks for a new name and renames the entry in place. */
  async function promptRename(entry) {
    const name = window.prompt(t("files.ui.promptRename", { name: entry.name }), entry.name);
    if (!name || name === entry.name || state.busy) return;
    try {
      await renameEntry(state.source, entry.path, name.trim());
      notify(t("files.ui.renamed", { from: entry.name, to: name.trim() }));
      await refresh();
    } catch (error) {
      notifyError(error);
    }
  }

  /** Confirms and deletes one file or folder. */
  async function confirmDelete(entry) {
    if (!window.confirm(t("files.ui.confirmDelete", { name: entry.name })) || state.busy) return;
    try {
      await deleteEntry(state.source, entry.path);
      notify(t("files.ui.deleted", { name: entry.name }));
      await refresh();
    } catch (error) {
      notifyError(error);
    }
  }

  /** Opens a directory (navigate) or dispatches files by kind. */
  async function openEntry(entry) {
    if (entry.isDir) {
      goTo(entry.path);
      return;
    }
    const kind = fileKind(entry.name);
    if (kind === "binary") {
      startDownload(entry);
      return;
    }
    if (kind === "gcode") {
      await openAsProgram(entry);
      return;
    }
    if (kind === "stock") {
      await openAsStock(entry);
      return;
    }
    await openAsText(entry);
  }

  /** Loads a G-code file as the active program and shows its toolpath. */
  async function openAsProgram(entry) {
    try {
      const { name, content } = await fetchContent(state.source, entry.path);
      const program = await invokeOrFetch(
        "load_program",
        { fileName: name || entry.name, content },
        "/api/program",
        "POST",
      );
      await app.program.showProgram(program.name, program.content);
    } catch (error) {
      notifyError(error);
    }
  }

  /** Parses a 3D file and shows it as the stock mesh in the scene. */
  async function openAsStock(entry) {
    try {
      const bytes = await fetchBytes(state.source, entry.path);
      const extension = entry.name.split(".").pop()?.toLowerCase();
      let mesh;
      if (extension === "stl") {
        mesh = parseStl(bytes);
      } else if (extension === "obj") {
        mesh = finalizeStockMesh(parseObj(new TextDecoder().decode(bytes)));
      } else if (extension === "json") {
        mesh = parseThreeJsJson(new TextDecoder().decode(bytes));
      } else {
        throw new Error(t("stock.error.unsupportedFormat"));
      }
      app.scene.applyStockMesh(mesh, entry.name);
    } catch (error) {
      notifyError(error);
    }
  }

  /** Opens a text file in the editor dialog with a save-back to its origin. */
  async function openAsText(entry) {
    const dialog = document.querySelector("#program-editor-dialog");
    if (!dialog) return;
    try {
      const { name, content } = await fetchContent(state.source, entry.path);
      const fileName = name || entry.name;
      setProgramEditorBaseline(content);
      // Snapshot the origin so later navigation does not retarget the save.
      setProgramEditorSaveTarget({
        name: fileName,
        save: (text) => uploadFile(state.source, dirOf(entry.path), fileName, text),
      });
      dialog.showModal();
    } catch (error) {
      notifyError(error);
    }
  }

  // Visibility follows the sidebar tabs: both tabs share `#sidebar-content`,
  // so the panel is hidden for MACROS and the placeholder keeps working.
  const filesTab = document.querySelector("#files-tab");
  const macrosTab = document.querySelector("#macros-tab");

  /** Shows or hides the panel; showing triggers a fresh load. */
  function setVisible(visible) {
    root.hidden = !visible;
    if (visible) refresh();
  }

  filesTab?.addEventListener("click", () => setVisible(true));
  macrosTab?.addEventListener("click", () => setVisible(false));

  // Re-render translated labels after a runtime locale switch; the backend
  // labels and error texts follow `lang` automatically on the next refresh.
  window.addEventListener("cnc-locale-changed", () => {
    if (!root.hidden) refresh();
  });

  setVisible(filesTab ? filesTab.getAttribute("aria-selected") === "true" : true);
}
