import { t } from "./i18n.js";

/** Storage key for user-assigned global shortcut bindings. */
export const SHORTCUT_STORAGE_KEY = "cnc-ui-shortcuts-v1";
/** Storage key for cross-layout physical-key matching preference. */
export const SHORTCUT_LAYOUT_STORAGE_KEY = "cnc-ui-shortcuts-layout-independent-v1";

/**
 * Defines supported global shortcuts; Abort is mandatory and cannot be reassigned or removed.
 */
/** Supported global actions and their default key chords; abort is mandatory. */
export const SHORTCUT_ACTIONS = [
  { id: "run", labelKey: "shortcuts.action.run", defaultKey: "Ctrl+Enter" },
  { id: "pause", labelKey: "shortcuts.action.pause", defaultKey: "Ctrl+P" },
  { id: "abort", labelKey: "shortcuts.action.abort", defaultKey: "Escape", required: true },
  { id: "viewerReset", labelKey: "shortcuts.action.viewerReset", defaultKey: "R" },
  { id: "toggleFiles", labelKey: "shortcuts.action.toggleFiles", defaultKey: "Ctrl+Shift+F" },
  { id: "togglePosition", labelKey: "shortcuts.action.togglePosition", defaultKey: "Ctrl+Shift+1" },
  { id: "toggleJog", labelKey: "shortcuts.action.toggleJog", defaultKey: "Ctrl+Shift+2" },
  { id: "toggleViewer", labelKey: "shortcuts.action.toggleViewer", defaultKey: "Ctrl+Shift+3" },
  { id: "toggleMacros", labelKey: "shortcuts.action.toggleMacros", defaultKey: "Ctrl+Shift+4" },
  { id: "toggleConsole", labelKey: "shortcuts.action.toggleConsole", defaultKey: "Ctrl+Shift+5" },
];

/** Reads whether keyboard shortcuts match by physical key across keyboard layouts. */
export function isLayoutIndependent() {
  try { return localStorage.getItem(SHORTCUT_LAYOUT_STORAGE_KEY) !== "false"; }
  catch { return true; }
}

/** Converts one keyboard event to a stable shortcut chord using physical or typed key identity. */
export function eventChord(event, layoutIndependent = isLayoutIndependent()) {
  if (["Control", "Alt", "Shift", "Meta"].includes(event.key)) return null;
  const parts = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Meta");
  const physicalCode = event.code || "";
  const key = layoutIndependent
    ? physicalCode.startsWith("Key") ? physicalCode.slice(3) : physicalCode.startsWith("Digit") ? physicalCode.slice(5) : physicalCode || event.key.toLocaleUpperCase()
    : event.key.length === 1 ? event.key.toLocaleUpperCase() : event.key;
  parts.push(key);
  return parts.join("+");
}

/** Loads validated local bindings, falling back to the built-in map for malformed or conflicting data. */
export function loadShortcutMap() {
  const defaults = Object.fromEntries(SHORTCUT_ACTIONS.map(({ id, defaultKey }) => [id, defaultKey]));
  try {
    const saved = JSON.parse(localStorage.getItem(SHORTCUT_STORAGE_KEY) || "{}");
    const candidate = { ...defaults, ...saved, abort: "Escape" };
    if (Object.keys(saved).some((id) => !SHORTCUT_ACTIONS.some((action) => action.id === id))) return defaults;
    if (SHORTCUT_ACTIONS.some(({ id, required }) => !required && (typeof candidate[id] !== "string" || !candidate[id].trim()))) return defaults;
    const values = Object.values(candidate).map((value) => value.toLowerCase());
    return new Set(values).size === values.length ? candidate : defaults;
  } catch { return defaults; }
}

/** Assigns one non-mandatory action a unique chord and emits the new map on success. */
export function assignShortcut(actionId, chord) {
  const action = SHORTCUT_ACTIONS.find((item) => item.id === actionId);
  if (!action || action.required || !chord || chord.length > 64) return { ok: false, conflict: "invalid" };
  const bindings = loadShortcutMap();
  const collision = Object.entries(bindings).find(([id, value]) => id !== actionId && value.toLowerCase() === chord.toLowerCase());
  if (collision) return { ok: false, conflict: collision[0] };
  bindings[actionId] = chord;
  try { localStorage.setItem(SHORTCUT_STORAGE_KEY, JSON.stringify(bindings)); }
  catch { return { ok: false, conflict: "storage" }; }
  window.dispatchEvent(new CustomEvent("cnc-shortcuts-changed", { detail: { bindings } }));
  return { ok: true, bindings };
}

/** Initializes the localized shortcut editor and its read-only header summary. */
export function initShortcutSettings() {
  const list = document.querySelector("#shortcut-list");
  const layoutIndependent = document.querySelector("#shortcut-layout-independent");
  if (!list) return { render() {} };

  const infoButton = document.querySelector("#shortcut-info-button");
  const infoPopover = document.querySelector("#shortcut-info-popover");
  const infoList = document.querySelector("#shortcut-info-list");

  /** Renders the compact read-only shortcut summary from the active bindings. */
  function renderInfo() {
    if (!infoList) return;
    infoList.replaceChildren();
    const bindings = loadShortcutMap();
    for (const action of SHORTCUT_ACTIONS) {
      const row = document.createElement("p");
      const label = document.createElement("span");
      const chord = document.createElement("kbd");
      label.textContent = t(action.labelKey);
      chord.textContent = bindings[action.id];
      row.append(label, chord);
      infoList.append(row);
    }
  }

  /** Opens or closes the shortcut summary and synchronizes its accessible state. */
  function setInfoOpen(open) {
    if (!infoPopover || !infoButton) return;
    infoPopover.hidden = !open;
    infoButton.setAttribute("aria-expanded", String(open));
    if (open) renderInfo();
  }

  /** Renders each configurable action and wires key capture for its binding. */
  function render() {
    list.replaceChildren();
    const bindings = loadShortcutMap();
    for (const action of SHORTCUT_ACTIONS) {
      const row = document.createElement("div");
      row.className = "shortcut-row";
      const label = document.createElement("span");
      label.textContent = t(action.labelKey);
      const button = document.createElement("button");
      button.type = "button";
      button.disabled = Boolean(action.required);
      button.textContent = bindings[action.id];
      if (action.required) button.title = t("shortcuts.abortRequired");
      if (!action.required) {
        let capturing = false;
        /** Cancels key capture and restores the current persisted chord. */
        function stopCapture() {
          if (!capturing) return;
          capturing = false;
          window.removeEventListener("keydown", capture, true);
          button.textContent = loadShortcutMap()[action.id];
        }
        /** Captures one eligible key chord without intercepting text entry. */
        function capture(event) {
          if (event.target instanceof HTMLElement && event.target.closest("input,textarea,select,[contenteditable='true']")) return;
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopImmediatePropagation();
            stopCapture();
            return;
          }
          const chord = eventChord(event, layoutIndependent?.checked ?? isLayoutIndependent());
          if (!chord) return;
          event.preventDefault();
          event.stopImmediatePropagation();
          capturing = false;
          window.removeEventListener("keydown", capture, true);
          const result = assignShortcut(action.id, chord);
          button.textContent = result.ok ? chord : t("shortcuts.assignmentFailed", { conflict: result.conflict });
          if (result.ok) render();
        }
        button.addEventListener("click", () => {
          if (capturing) return;
          capturing = true;
          button.textContent = t("shortcuts.pressKey");
          window.addEventListener("keydown", capture, true);
        });
      }
      row.append(label, button);
      list.append(row);
    }
  }

  const settingsTabs = [
    [document.querySelector("#settings-general-tab"), document.querySelector("#settings-general-panel")],
    [document.querySelector("#settings-shortcuts-tab"), document.querySelector("#settings-shortcuts-panel")],
    [document.querySelector("#settings-machine-data-tab"), document.querySelector("#settings-machine-data-panel")],
  ];
  /** Selects one Settings tab and synchronizes its panel and accessible state. */
  function selectTab(index) {
    for (const [tab, panel] of settingsTabs) {
      if (!tab || !panel) continue;
      const selected = settingsTabs[index]?.[0] === tab;
      panel.hidden = !selected;
      tab.classList.toggle("active", selected);
      tab.setAttribute("aria-selected", String(selected));
    }
  }
  settingsTabs.forEach(([tab], index) => tab?.addEventListener("click", () => selectTab(index)));
  window.addEventListener("cnc-open-settings-shortcuts", () => {
    const dialog = document.querySelector("#ui-settings-dialog");
    if (dialog && !dialog.open) dialog.showModal();
    selectTab(1);
  });
  window.addEventListener("cnc-open-settings-machine-data", () => {
    const dialog = document.querySelector("#ui-settings-dialog");
    if (dialog && !dialog.open) dialog.showModal();
    selectTab(2);
  });

  try { if (layoutIndependent) layoutIndependent.checked = isLayoutIndependent(); } catch { /* Storage may be blocked. */ }
  layoutIndependent?.addEventListener("change", () => {
    try { localStorage.setItem(SHORTCUT_LAYOUT_STORAGE_KEY, String(layoutIndependent.checked)); } catch { /* Current page behavior still updates. */ }
    window.dispatchEvent(new CustomEvent("cnc-shortcuts-changed", { detail: { bindings: loadShortcutMap(), layoutIndependent: layoutIndependent.checked } }));
  });
  document.querySelector("#shortcut-reset")?.addEventListener("click", () => {
    try { localStorage.removeItem(SHORTCUT_STORAGE_KEY); } catch { /* Page defaults still apply if storage is blocked. */ }
    render();
    window.dispatchEvent(new CustomEvent("cnc-shortcuts-changed", { detail: { bindings: loadShortcutMap(), layoutIndependent: isLayoutIndependent() } }));
  });
  infoButton?.addEventListener("click", () => setInfoOpen(infoPopover?.hidden ?? true));
  document.querySelector("#shortcut-info-settings")?.addEventListener("click", () => {
    setInfoOpen(false);
    window.dispatchEvent(new CustomEvent("cnc-open-settings-shortcuts"));
  });
  document.addEventListener("pointerdown", (event) => {
    if (!infoPopover?.hidden && !infoPopover?.contains(event.target) && !infoButton?.contains(event.target)) setInfoOpen(false);
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && infoPopover && !infoPopover.hidden) setInfoOpen(false);
  });
  window.addEventListener("cnc-shortcuts-changed", renderInfo);
  window.addEventListener("cnc-locale-changed", () => { render(); renderInfo(); });
  selectTab(0);
  render();
  renderInfo();
  return { render };
}
