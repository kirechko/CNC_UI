import { invokeOrFetch } from "./api.js";

export function initUiMessageLogger({ elements, getStatus }) {
  const lastLoggedMessages = new WeakMap();

  for (const [element, source] of elements) {
    if (!element) continue;

    lastLoggedMessages.set(element, element.textContent.trim());

    const recordMessage = async () => {
      const message = element.textContent.trim();
      if (!message || lastLoggedMessages.get(element) === message) return;
      lastLoggedMessages.set(element, message);

      const status = getStatus();
      // Messages already shown as the controller status are not duplicated in the journal.
      if (
        source === "interface" &&
        status &&
        [status.message, status.error].filter(Boolean).includes(message)
      ) {
        return;
      }

      // Severity is an internal English identifier (alarm/error/warning/info).
      const severity = /^(alarm|авария|emergency stop)\b/i.test(message)
        ? "alarm"
        : /^(error|failed|failure|ошибка|не удалось)\b/i.test(message)
          ? "error"
          : /^(warning|предупреждение)\b/i.test(message)
            ? "warning"
            : "info";
      if (severity === "info") return;

      try {
        await invokeOrFetch(
          "log_message",
          { severity, source, message },
          "/api/log",
          "POST",
        );
      } catch (error) {
        console.warn("Failed to write interface message to the journal:", error);
      }
    };

    new MutationObserver(recordMessage).observe(element, {
      childList: true,
      characterData: true,
      subtree: true,
    });
  }
}
