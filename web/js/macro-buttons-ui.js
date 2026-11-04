/**
 * Quick-run macro button controls, including their local assignment storage.
 * The caller owns the shared macro state and decides when to rerender the buttons.
 */

/** Reads saved macro-button assignments without failing in restricted storage contexts. */
export function loadMacroButtons() {
  try {
    const value = JSON.parse(localStorage.getItem("cnc-ui-macro-buttons") || "[]");
    return Array.isArray(value) ? value.filter((id) => typeof id === "string") : [];
  } catch {
    return [];
  }
}

/** Creates quick-run button behavior around the caller's existing macro state and DOM. */
export function createMacroButtonsUi({
  state,
  macroButtons,
  macroButtonPicker,
  macroButtonPickerForm,
  macroButtonPickerSelect,
  addMacroButton,
  panelStatus,
  t,
}) {
  /** Persists button assignments locally; macro definitions remain in the shared database. */
  function saveMacroButtons() {
    try {
      localStorage.setItem("cnc-ui-macro-buttons", JSON.stringify(state.buttons));
    } catch {
      panelStatus.textContent = t("macros.buttons.storageError");
    }
  }

  /** Renders one run button per configured macro and controls for changing assignments. */
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
      // Shortcut and standard list runners share the same window event contract.
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

  /** Adds a macro to the configured quick-run buttons when it is not already assigned. */
  function assignMacroButton(macroId) {
    const macro = state.macros.find((item) => item.id === macroId);
    if (!macro || state.buttons.includes(macro.id)) return;
    state.buttons.push(macro.id);
    saveMacroButtons();
    renderMacroButtons();
  }

  /** Opens the accessible picker for an available quick-run macro. */
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

  /** Applies the picker selection and closes the dialog without changing canceled assignments. */
  function submitMacroButtonPicker(event) {
    event.preventDefault();
    if (event.submitter?.value === "add") {
      assignMacroButton(macroButtonPickerSelect.value);
    }
    macroButtonPicker.close();
  }

  // Keep listener installation at the point where the main UI binds its other controls.
  return { renderMacroButtons, addConfiguredMacroButton, submitMacroButtonPicker };
}
