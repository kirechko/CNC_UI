import { deleteLocalToolType, loadLocalMachineData, saveLocalToolType } from "./local-machine-data.js";
import { t } from "./i18n.js";

/** Builds the type-management view from DOM elements supplied by the existing settings dialog. */
export function createToolTypeManager({ manager, list, form, openButton, closeButton, editorView, onChange, onStatus }) {
  let records = [];

  /** Returns localized text or the provided English fallback when a translation is missing. */
  function localized(key, fallback) {
    const value = t(key);
    return value === key ? fallback : value;
  }

  /** Loads and publishes the current independent tool type list. */
  async function reload() {
    records = await loadLocalMachineData("toolTypes");
    render();
    onChange(records);
  }

  /** Fills the editor with one type record or resets it for a new one. */
  function fill(record = null) {
    form.reset();
    form.elements.id.value = record?.id ?? "";
    form.elements.name.value = record?.name ?? "";
    form.elements.cuttingType.value = record?.cuttingType ?? "milling";
    form.elements.geometryModel.value = record?.geometryModel ?? "cylindrical";
  }

  /** Creates type rows with text-only names and explicit edit/delete buttons. */
  function render() {
    list.replaceChildren();
    for (const record of records) {
      const row = document.createElement("div");
      row.className = "machine-data-row";
      const summary = document.createElement("span");
      summary.textContent = `${record.name} · ${record.cuttingType} · ${record.geometryModel}`;
      const edit = document.createElement("button");
      edit.type = "button";
      edit.textContent = localized("machineData.edit", "Edit");
      edit.addEventListener("click", () => fill(record));
      const remove = document.createElement("button");
      remove.type = "button";
      remove.textContent = localized("machineData.delete", "Delete");
      remove.addEventListener("click", async () => {
        if (!window.confirm(`${localized("machineData.delete", "Delete")} ${record.name}?`)) return;
        try {
          await deleteLocalToolType(record.id);
          await reload();
          onStatus(localized("toolTypes.deleted", "Tool type deleted."));
        } catch (error) {
          onStatus(error.message);
        }
      });
      row.append(summary, edit, remove);
      list.append(row);
    }
    if (records.length === 0) {
      const empty = document.createElement("p");
      empty.textContent = localized("toolTypes.empty", "No tool types configured.");
      list.append(empty);
    }
  }

  /** Persists the form and refreshes the list and tool-type selector. */
  async function save(event) {
    event.preventDefault();
    const id = form.elements.id.value || `type-${crypto.randomUUID()}`;
    try {
      await saveLocalToolType({
        id,
        name: form.elements.name.value,
        cuttingType: form.elements.cuttingType.value,
        geometryModel: form.elements.geometryModel.value,
      });
      await reload();
      fill();
      onStatus(localized("toolTypes.saved", "Tool type saved."));
    } catch (error) {
      onStatus(error.message);
    }
  }

  /** Opens the category manager and reloads its SQLite-backed records. */
  function open() {
    manager.hidden = false;
    editorView.hidden = true;
    fill();
    void reload().catch((error) => onStatus(error.message));
  }

  /** Closes category editing and restores the existing tool/profile editor. */
  function close() {
    manager.hidden = true;
    editorView.hidden = false;
  }

  form.addEventListener("submit", (event) => void save(event));
  window.addEventListener("cnc-locale-changed", () => {
    render();
    for (const option of form.elements.cuttingType.options) {
      const key = `toolTypes.cutting.${option.value}`;
      option.textContent = localized(key, option.value);
    }
    for (const option of form.elements.geometryModel.options) {
      const key = `toolTypes.geometry.${option.value}`;
      option.textContent = localized(key, option.value);
    }
  });
  openButton?.addEventListener("click", open);
  closeButton?.addEventListener("click", close);
  return { reload };
}
