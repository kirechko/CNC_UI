// Machine-console log bridge: writes command/G-code entries to the backend
// message log via the Tauri `log_message` command or the HTTP `/api/log`
// endpoint. Extracted from main.js so both the program editor and the
// command console can share the exact same logging path.
import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/**
 * Persist one machine-console entry (severity + human-readable message).
 * Failures are only warned to the browser console: losing a log record must
 * never break the UI action that produced it.
 * @param {string} severity Log severity understood by the backend ("info", "warning", "error").
 * @param {string} message Message text shown in the message log.
 */
export async function logMachineConsoleEvent(severity, message) {
  try {
    await invokeOrFetch(
      "log_message",
      { severity, source: "commands and G-code", message },
      "/api/log",
      "POST",
    );
  } catch (error) {
    console.warn(t("ui.log.writeFailed"), error);
  }
}

/**
 * Format an error as a machine-console error entry, persist it, and return
 * the formatted text so callers can also surface it to the user.
 * @param {string} context Human-readable action description prefixed to the error.
 * @param {unknown} error The caught error (Error or arbitrary value).
 * @returns {Promise<string>} The composed message that was written to the log.
 */
export async function logMachineConsoleError(context, error) {
  const message = `${context}: ${error instanceof Error ? error.message : String(error)}`;
  await logMachineConsoleEvent("error", message);
  return message;
}
