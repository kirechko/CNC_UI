// ui-settings.js
// Interface settings panel: offers theme selection (dark / light / green /
// gray) and language selection. The chosen theme is persisted in localStorage
// and applied by setting `data-theme` on <html>, which activates the matching
// token override block in web/css/theme-*.css. Switching the theme dispatches a
// "cnc-theme-changed" event so every canvas renderer can repaint itself with
// the new palette (canvas colors cannot follow CSS variables automatically).
// Switching the language delegates to i18n.switchLocale(), which persists the
// choice, re-translates the document and dispatches "cnc-locale-changed" for
// dynamically rendered text (compact labels, jog captions, ...).

import { getLocale, switchLocale } from "./i18n.js";

/** localStorage key holding the user's theme choice. */
export const THEME_STORAGE_KEY = "cnc-ui-theme";

/** Canonical theme identifiers; must match web/css/theme-*.css + the dialog markup. */
export const THEMES = ["dark", "light", "green", "gray"];

/** Theme used when nothing is stored (also the stylesheet's default palette). */
export const DEFAULT_THEME = "dark";

/**
 * Reads the persisted theme, falling back to the default for unknown values
 * or when storage is unavailable (private browsing, Tauri webview quirks).
 *
 * @returns {string} One of THEMES.
 */
export function readStoredTheme() {
  try {
    const stored = localStorage.getItem(THEME_STORAGE_KEY);
    return THEMES.includes(stored) ? stored : DEFAULT_THEME;
  } catch {
    return DEFAULT_THEME;
  }
}

/**
 * Applies a theme to the document and notifies listeners.
 *
 * @param {string} theme - One of THEMES; unknown values fall back to DEFAULT_THEME.
 * @param {{persist?: boolean}} [options] - When persist is true (default) the
 *   choice is written to localStorage so the next start restores it.
 * @returns {string} The theme actually applied.
 */
export function applyTheme(theme, { persist = true } = {}) {
  const safe = THEMES.includes(theme) ? theme : DEFAULT_THEME;
  document.documentElement.dataset.theme = safe;
  if (persist) {
    try {
      localStorage.setItem(THEME_STORAGE_KEY, safe);
    } catch {
      // Storage may throw in sandboxed contexts; the theme still applies live.
    }
  }
  // Canvas renderers repaint on this event (see theme-colors.js cache reset).
  window.dispatchEvent(new CustomEvent("cnc-theme-changed", { detail: { theme: safe } }));
  return safe;
}

/**
 * Wires the header settings button and the settings dialog.
 *
 * The dialog contains one radio input per theme inside `.theme-option` labels.
 * Opening the dialog syncs the radios with the active theme; picking a radio
 * applies + persists the theme immediately (no extra "OK" step).
 *
 * @param {object} [options] - Element handles (queried by default selectors).
 * @param {Element|null} [options.button] - Gear button that opens the dialog.
 * @param {Element|null} [options.dialog] - The <dialog> element itself.
 * @param {Element|null} [options.closeButton] - Close control inside the dialog.
 * @param {string} [options.radioSelector] - Selector for the theme radios.
 * @returns {{open: () => void, close: () => void}} Control handles for tests.
 */
export function initUiSettings({
  button = document.querySelector("#ui-settings-button"),
  dialog = document.querySelector("#ui-settings-dialog"),
  closeButton = document.querySelector("#ui-settings-close"),
  radioSelector = 'input[name="ui-theme"]',
  languageSelect = document.querySelector("#ui-language"),
} = {}) {
  if (!button || !dialog) return { open: () => {}, close: () => {} };

  const radios = Array.from(dialog.querySelectorAll(radioSelector));

  /** Marks the radio matching the current theme as checked. */
  function syncRadios() {
    const current = document.documentElement.dataset.theme || DEFAULT_THEME;
    for (const radio of radios) radio.checked = radio.value === current;
  }

  /** Selects the option matching the currently active locale. */
  function syncLanguage() {
    if (languageSelect) languageSelect.value = getLocale();
  }

  /** Brings every control in the dialog in line with the live state. */
  function syncAll() {
    syncRadios();
    syncLanguage();
  }

  // The inline <head> script already applied the stored theme before first
  // paint; adopt the value into the dataset (and re-dispatch) so listeners
  // such as the canvas repaint hook are registered against a known theme.
  syncAll();

  button.addEventListener("click", () => {
    syncAll();
    dialog.showModal();
  });

  if (closeButton) {
    closeButton.addEventListener("click", () => dialog.close());
  }

  for (const radio of radios) {
    radio.addEventListener("change", () => {
      if (radio.checked) applyTheme(radio.value);
    });
  }

  if (languageSelect) {
    languageSelect.addEventListener("change", async () => {
      // switchLocale returns the locale that was actually applied (English on
      // dictionary load failure); re-sync so the select never lies.
      const applied = await switchLocale(languageSelect.value);
      languageSelect.value = applied;
    });
  }

  return {
    open: () => {
      syncAll();
      dialog.showModal();
    },
    close: () => dialog.close(),
  };
}
