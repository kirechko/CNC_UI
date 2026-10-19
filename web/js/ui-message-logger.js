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
      if (
        source === "интерфейс" &&
        status &&
        [status.message, status.error].filter(Boolean).includes(message)
      ) {
        return;
      }

      const severity = /^(ошибка|не удалось|error|failed|failure)\b/i.test(message)
        ? "ошибка"
        : "информация";

      try {
        await invokeOrFetch(
          "log_message",
          { severity, source, message },
          "/api/log",
          "POST",
        );
      } catch (error) {
        console.warn("Не удалось записать сообщение интерфейса в журнал:", error);
      }
    };

    new MutationObserver(recordMessage).observe(element, {
      childList: true,
      characterData: true,
      subtree: true,
    });
  }
}
