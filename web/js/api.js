import { t } from "./i18n.js";

/**
 * Calls a backend command. Uses the Tauri `invoke` bridge when running inside
 * the desktop shell, and falls back to an HTTP request to `endpoint` otherwise.
 * An empty response body is returned as `null`.
 * @param {string} command Tauri command name.
 * @param {unknown} args Arguments for the command; sent as a JSON body over HTTP when not null.
 * @param {string} endpoint HTTP path used in browser mode.
 * @param {string} [method="GET"] HTTP method used in browser mode.
 * @returns {Promise<unknown>} Parsed JSON response, or null when the body is empty.
 * @throws {Error} When the HTTP status is not OK or the body is not valid JSON.
 */
export async function invokeOrFetch(command, args, endpoint, method = "GET") {
  const invoke = window.__TAURI__?.core?.invoke;
  if (invoke) return invoke(command, args);

  const response = await fetch(endpoint, {
    method,
    cache: "no-store",
    ...(args ? {
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(args),
    } : {}),
  });
  const responseText = await response.text();
  if (!response.ok) {
    throw new Error(responseText || `HTTP ${response.status}`);
  }
  if (!responseText.trim()) return null;

  try {
    return JSON.parse(responseText);
  } catch {
    throw new Error(t("api.invalidJson"));
  }
}
