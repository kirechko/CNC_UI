// Internationalization (i18n) helper for the web UI.
//
// Locale dictionaries are plain JSON files stored in `web/locales/<locale>.json`.
// Each file maps flat message keys (for example "sidebar.macros.description")
// to translated strings. Strings may contain named placeholders such as
// "{axis}" which are replaced by `t(key, { axis: "X" })`.
//
// The active locale is chosen in this order of precedence:
//   1. the `?lang=<locale>` URL parameter (explicit, one-off override),
//   2. the locale persisted in `localStorage["cnc-ui-locale"]` (settings panel),
//   3. `DEFAULT_LOCALE` (English).
// Keys missing from the active locale are resolved from the English dictionary,
// and if they are missing there too the key itself is returned so that gaps are
// visible in the UI instead of failing silently.

/** Locale used when neither the URL nor the stored preference provides one. */
export const DEFAULT_LOCALE = "en";

/**
 * Locales offered in the interface-settings panel. `name` is written in its own
 * language on purpose: the picker must remain usable even before it is
 * translated, so option labels are never passed through `t()`.
 */
export const LOCALES = Object.freeze([
  { code: "en", name: "English" },
  { code: "ru", name: "Русский" },
  { code: "zh", name: "中文" },
  { code: "fa", name: "فارسی" },
]);

/** localStorage key that remembers the locale picked in the settings dialog. */
export const LOCALE_STORAGE_KEY = "cnc-ui-locale";

/** Active dictionary, populated by `loadLocale`. */
let activeMessages = {};
/** English dictionary used for fallback lookups. */
let fallbackMessages = {};
/** Name of the currently loaded locale. */
let currentLocale = DEFAULT_LOCALE;

/**
 * Reads the locale previously chosen in the settings dialog.
 * @returns {string|null} Stored locale identifier, or null when absent/invalid.
 */
export function readStoredLocale() {
  try {
    const stored = window.localStorage.getItem(LOCALE_STORAGE_KEY);
    // Only accept identifiers that look sane so the value cannot alter the
    // fetch path; unknown-but-well-formed codes still fall back gracefully in
    // loadLocale(), so we do not need to match them against LOCALES here.
    return stored && /^[a-zA-Z0-9_-]+$/.test(stored) ? stored : null;
  } catch (error) {
    // Private-mode browsers can throw on localStorage access; ignore it.
    return null;
  }
}

/**
 * Resolves the locale to activate at boot: the `?lang=xx` URL parameter wins,
 * then the stored preference, then the default locale.
 * @returns {string} Locale identifier such as "en" or "ru".
 */
export function resolveRequestedLocale() {
  const requested = new URLSearchParams(window.location.search).get("lang");
  // Restrict to simple identifiers so the value cannot alter the file path.
  if (requested && /^[a-zA-Z0-9_-]+$/.test(requested)) {
    return requested;
  }
  return readStoredLocale() ?? DEFAULT_LOCALE;
}

/**
 * Fetches a locale dictionary.
 *
 * Two runtime environments must be supported:
 *  - Browser mode (axum/static server): dictionaries are plain files served
 *    from `web/locales/`, so a regular `fetch()` works.
 *  - Desktop mode (Tauri shell): the frontend is served from a custom protocol
 *    where `fetch()` of asset files may be blocked or return opaque errors, so
 *    the dictionaries are requested from the Rust side, which has them
 *    embedded in the binary (`src/i18n.rs` -> `get_ui_dictionary` command).
 * The Tauri branch falls through to `fetch()` when the shell does not know
 * the command yet (older desktop builds), keeping behavior backwards compatible.
 * @param {string} locale Locale identifier.
 * @returns {Promise<Record<string, string>>} Parsed dictionary.
 * @throws {Error} When neither the invoke bridge nor HTTP can deliver the file.
 */
async function fetchDictionary(locale) {
  const invoke = window.__TAURI__?.core?.invoke;
  if (invoke) {
    try {
      // The command returns the raw embedded JSON source as a string.
      return JSON.parse(await invoke("get_ui_dictionary", { language: locale }));
    } catch (error) {
      // Older binaries lack the command; still try static files below.
      console.warn("Tauri dictionary command failed, trying HTTP fallback:", error);
    }
  }
  const url = new URL(`../locales/${locale}.json`, import.meta.url);
  const response = await fetch(url, { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`Locale "${locale}" not found (HTTP ${response.status})`);
  }
  return response.json();
}

/**
 * Loads the requested locale and the English fallback dictionary.
 * If the requested locale cannot be loaded, English is used and a warning is logged.
 * @param {string} [locale] Locale to load; defaults to the value from the URL.
 * @returns {Promise<string>} The locale that was actually activated.
 */
export async function loadLocale(locale = resolveRequestedLocale()) {
  try {
    fallbackMessages = await fetchDictionary(DEFAULT_LOCALE);
  } catch (error) {
    // Even the English fallback is unreachable (e.g. a desktop build without
    // the dictionary command and a blocked asset protocol). Keep booting with
    // an empty fallback so `t()` shows raw keys instead of killing main.js.
    console.warn("Failed to load the English fallback dictionary:", error);
    fallbackMessages = {};
  }
  if (locale === DEFAULT_LOCALE) {
    activeMessages = fallbackMessages;
    currentLocale = DEFAULT_LOCALE;
    return currentLocale;
  }
  try {
    activeMessages = await fetchDictionary(locale);
    currentLocale = locale;
  } catch (error) {
    console.warn("Failed to load locale, falling back to English:", error);
    activeMessages = fallbackMessages;
    currentLocale = DEFAULT_LOCALE;
  }
  return currentLocale;
}

/**
 * Returns the identifier of the active locale.
 * @returns {string} Active locale identifier.
 */
export function getLocale() {
  return currentLocale;
}

/**
 * Switches the UI language at runtime: persists the choice, reloads the
 * dictionaries, re-translates the whole document, keeps the address bar in
 * sync and notifies the rest of the app with a `cnc-locale-changed` event so
 * dynamically rendered text (compact button labels, jog step captions, …) can
 * be re-rendered in the new language.
 * @param {string} locale Locale code to activate, e.g. "ru".
 * @returns {Promise<string>} The locale that was actually applied (falls back
 *   to English when the requested dictionary cannot be loaded).
 */
export async function switchLocale(locale) {
  if (!/^[a-zA-Z0-9_-]+$/.test(locale)) {
    return currentLocale;
  }
  try {
    window.localStorage.setItem(LOCALE_STORAGE_KEY, locale);
  } catch (error) {
    // Storage may be unavailable (private mode); the switch still proceeds.
  }
  const applied = await loadLocale(locale);

  // Re-translate all static markup with the freshly loaded dictionary.
  applyTranslations(document);

  // Mirror the active choice into the URL so a manual reload keeps it even if
  // localStorage is cleared. "en" is the default, so its parameter is dropped.
  // The update is best-effort: custom protocols used by the Tauri desktop
  // shell can reject history writes with a SecurityError, and the language
  // switch must still finish (re-translation + event dispatch) when that
  // happens, otherwise the select would snap back to English for no reason.
  try {
    const url = new URL(window.location.href);
    if (applied === DEFAULT_LOCALE) {
      url.searchParams.delete("lang");
    } else {
      url.searchParams.set("lang", applied);
    }
    window.history.replaceState(null, "", url);
  } catch (error) {
    // Storage + dictionaries are already in sync; skip the address bar mirror.
  }

  window.dispatchEvent(new CustomEvent("cnc-locale-changed", { detail: { locale: applied } }));
  return applied;
}

/**
 * Translates a message key and substitutes named placeholders.
 * @param {string} key Flat message key, for example "jog.step.title".
 * @param {Record<string, string|number>} [params] Values for "{name}" placeholders.
 * @returns {string} Translated text, the English text, or the key if no text exists.
 */
export function t(key, params = {}) {
  const template = activeMessages[key] ?? fallbackMessages[key] ?? key;
  return template.replace(/\{(\w+)\}/g, (match, name) =>
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : match,
  );
}

/**
 * Applies translations to static markup using data attributes:
 * - `data-i18n` sets textContent,
 * - `data-i18n-title` sets the title attribute,
 * - `data-i18n-aria-label` sets aria-label,
 * - `data-i18n-placeholder` sets placeholder.
 * @param {ParentNode} [root=document] Root node whose descendants are translated.
 * @returns {void}
 */
export function applyTranslations(root = document) {
  root.querySelectorAll("[data-i18n]").forEach((element) => {
    element.textContent = t(element.dataset.i18n);
  });
  root.querySelectorAll("[data-i18n-title]").forEach((element) => {
    element.title = t(element.dataset.i18nTitle);
  });
  root.querySelectorAll("[data-i18n-aria-label]").forEach((element) => {
    element.setAttribute("aria-label", t(element.dataset.i18nAriaLabel));
  });
  root.querySelectorAll("[data-i18n-placeholder]").forEach((element) => {
    element.placeholder = t(element.dataset.i18nPlaceholder);
  });
  document.documentElement.lang = currentLocale;
}
