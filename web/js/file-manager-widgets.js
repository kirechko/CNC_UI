// Shared low-level building blocks for the file-manager panel UI: the inline
// SVG icon set, the icon-only button factory, byte/date formatters and the
// two status-message helpers. Extracted so `file-manager-ui.js` stays focused
// on state and layout while these pure utilities remain independently
// testable and reusable by future panels.
import { getLocale, t } from "./i18n.js";

/** Static inline-SVG path sets; every icon is stroke-only like the rest of the UI. */
export const ICONS = {
  up: '<path d="M12 19V5"/><path d="m5 12 7-7 7 7"/>',
  refresh: '<path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>',
  file: '<path d="M14 3v5h5"/><path d="M6 3h8l5 5v13H6Z"/><path d="M9 13h6M9 17h4"/>',
  open: '<path d="M15 3h6v6"/><path d="M10 14 21 3"/><path d="M18 13v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h7"/>',
  download: '<path d="M12 3v12"/><path d="m6 11 6 6 6-6"/><path d="M4 21h16"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/>',
  cut: '<circle cx="6" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M20 4 8.12 15.88"/><path d="M14.8 14.8 20 20"/><path d="M8.12 8.12 12 12"/>',
  paste: '<path d="M9 4h6v3H9Z"/><path d="M15 5h3a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2h3"/>',
  rename: '<path d="m4 20 4-1L20 7l-3-3L5 16Z"/><path d="m14 6 3 3"/>',
  remove: '<path d="M4 7h16"/><path d="M9 7V4h6v3"/><path d="m6 7 1 14h10l1-14"/>',
  newFolder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/><path d="M12 10v6M9 13h6"/>',
  upload: '<path d="M12 15V3"/><path d="m6 8 6-6 6 6"/><path d="M4 21h16"/>',
};

/**
 * Wraps one icon path set in a standard inline SVG element.
 * @param {string} name Icon key from `ICONS`.
 * @returns {string} SVG markup string.
 */
export function svg(name) {
  return `<svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" `
    + 'stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
    + `${ICONS[name]}</svg>`;
}

/**
 * Creates a small icon-only button with a translated title/aria-label.
 * @param {string} icon Icon key from `ICONS`.
 * @param {string} labelKey i18n key used for both title and aria-label.
 * @param {Function} onClick Click handler (may be async).
 * @param {boolean} [disabled=false] Initial disabled state.
 * @returns {HTMLButtonElement} The button, ready to be inserted into the DOM.
 */
export function iconButton(icon, labelKey, onClick, disabled = false) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "fm-icon-button";
  button.innerHTML = svg(icon);
  const label = t(labelKey);
  button.title = label;
  button.setAttribute("aria-label", label);
  button.disabled = disabled;
  button.addEventListener("click", () => onClick());
  return button;
}

/**
 * Formats a byte count with binary units (KiB/MiB/…).
 * @param {number} bytes Size in bytes.
 * @returns {string} Localized-looking magnitude plus unit.
 */
export function formatBytes(bytes) {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = Number(bytes) || 0;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const formatted = unit === 0 ? String(value) : value.toFixed(value >= 100 ? 0 : 1);
  return `${formatted} ${units[unit]}`;
}

/**
 * Formats a Unix-millisecond timestamp for the active locale.
 * @param {number} ms Epoch milliseconds.
 * @returns {string} Localized date+time, or "" when formatting fails.
 */
export function formatDate(ms) {
  try {
    return new Intl.DateTimeFormat(getLocale(), { dateStyle: "medium", timeStyle: "short" })
      .format(new Date(Number(ms)));
  } catch {
    return "";
  }
}

/**
 * Final directory of a `/`-prefixed path (`"/"` when there is none).
 * @param {string} path Entry path.
 * @returns {string} Directory path.
 */
export function dirOf(path) {
  const index = path.lastIndexOf("/");
  return index <= 0 ? "/" : path.slice(0, index);
}

/**
 * Writes a transient status line into the hidden `#message` node; the UI
 * message logger mirrors it into the backend log like any other status text.
 * @param {string} message Message to show.
 */
export function notify(message) {
  const node = document.querySelector("#message");
  if (node) node.textContent = message;
}

/**
 * Shows a translated backend/network error. Backend file-manager errors are
 * already localized through the `lang` parameter, so the text is used verbatim
 * inside the shared error prefix.
 * @param {unknown} error Thrown value.
 */
export function notifyError(error) {
  notify(t("common.errorPrefix", { message: error?.message ?? String(error) }));
}
