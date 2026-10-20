import { invokeOrFetch } from "./api.js";
import { parseLocalDate } from "./log-calendar-utils.js";

export function renderLatestLogLine(element, entry, emptyText) {
  if (!element) return;

  if (!entry) {
    element.textContent = emptyText;
    element.removeAttribute("title");
    return;
  }

  const metadata = document.createElement("span");
  metadata.className = "message-meta";
  metadata.textContent =
    `${new Date(entry.timestamp_ms).toLocaleString()} · ${entry.severity} · ${entry.source} · `;

  element.replaceChildren(metadata, document.createTextNode(entry.message));
  element.title = `${metadata.textContent}${entry.message}`;
}

function formatLogTimestamp(timestamp) {
  return new Date(timestamp).toLocaleString();
}

async function copyTextToClipboard(text) {
  if (window.isSecureContext && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text);
      return;
    } catch {
      // Если Clipboard API недоступен или запрещён, пробуем резервный способ.
    }
  }

  const activeElement = document.activeElement;
  const textarea = document.createElement("textarea");
  textarea.value = text;
  textarea.setAttribute("readonly", "");
  Object.assign(textarea.style, {
    position: "fixed",
    top: "0",
    left: "0",
    width: "1px",
    height: "1px",
    padding: "0",
    border: "0",
    opacity: "0.01",
    fontSize: "16px",
  });
  document.body.append(textarea);

  try {
    textarea.focus();
    textarea.select();
    textarea.setSelectionRange(0, textarea.value.length);
    if (!document.execCommand("copy")) {
      throw new Error("Браузер не разрешил копирование в буфер обмена");
    }
  } finally {
    textarea.remove();
    if (activeElement instanceof HTMLElement) activeElement.focus();
  }
}

export function initMessageLog({ getDateRange }) {
  const dialog = document.querySelector("#message-log-dialog");
  const rows = document.querySelector("#message-log-rows");
  const count = document.querySelector("#message-log-count");
  const pageSizeSelect = document.querySelector("#message-log-page-size");
  const copyPageButton = document.querySelector("#message-log-copy-page");
  let pageSize = Number(pageSizeSelect.value) || 100;
  let offset = 0;
  let sort = { column: null, direction: 0 };

  async function requestPage(pageOffset = offset) {
    const params = new URLSearchParams({
      descending: String(sort.direction !== 1),
      limit: String(pageSize),
      offset: String(pageOffset),
    });
    const severity = document.querySelector("#log-severity").value;
    const source = document.querySelector("#log-source").value.trim();
    const search = document.querySelector("#log-search").value.trim();
    const range = getDateRange();
    const startDate = parseLocalDate(range.start);
    const endDate = parseLocalDate(range.end || range.start);
    const [startHour, startMinute] = (range.startTime || "00:00").split(":").map(Number);
    const [endHour, endMinute] = (range.endTime || "23:59").split(":").map(Number);
    const timeFromMs = startDate
      ? new Date(
          startDate.getFullYear(),
          startDate.getMonth(),
          startDate.getDate(),
          startHour,
          startMinute,
        ).getTime()
      : null;
    const timeToMs = endDate
      ? new Date(
          endDate.getFullYear(),
          endDate.getMonth(),
          endDate.getDate(),
          endHour,
          endMinute + 1,
        ).getTime() - 1
      : null;

    if (sort.column && sort.direction !== 0) params.set("sort", sort.column);
    if (severity) params.set("severity", severity);
    if (source) params.set("source", source);
    if (search) params.set("search", search);
    if (timeFromMs !== null) params.set("time_from_ms", String(timeFromMs));
    if (timeToMs !== null) params.set("time_to_ms", String(timeToMs));

    if (window.__TAURI__?.core?.invoke) {
      return invokeOrFetch("get_log", {
        severity: severity || null,
        source: source || null,
        search: search || null,
        sort: sort.column && sort.direction !== 0 ? sort.column : null,
        descending: sort.direction !== 1,
        timeFromMs,
        timeToMs,
        limit: pageSize,
        offset: pageOffset,
      });
    }

    const response = await fetch(`/api/log?${params}`, { cache: "no-store" });
    if (!response.ok) {
      throw new Error((await response.text()) || `HTTP ${response.status}`);
    }
    return response.json();
  }


  async function refresh() {
    try {
      const page = await requestPage();
      rows.replaceChildren();

      for (const entry of page.entries) {
        const row = document.createElement("tr");
        const timestamp = document.createElement("td");
        timestamp.textContent = formatLogTimestamp(entry.timestamp_ms);

        const severity = document.createElement("td");
        severity.textContent = entry.severity;
        severity.className = entry.severity === "авария" ? "critical" : "";

        const source = document.createElement("td");
        source.textContent = entry.source;

        const message = document.createElement("td");
        message.textContent = entry.message;

        row.append(timestamp, severity, source, message);
        rows.append(row);
      }

      count.textContent =
        `${page.total} сообщений · ${page.total ? offset + 1 : 0}–${Math.min(offset + page.entries.length, page.total)}`;
      document.querySelector("#message-log-prev").disabled = offset <= 0;
      document.querySelector("#message-log-next").disabled =
        offset + page.entries.length >= page.total;
      copyPageButton.disabled = page.entries.length === 0;
    } catch (error) {
      count.textContent = `Не удалось загрузить журнал: ${error.message}`;
    }
  }

  function resetAndRefresh() {
    offset = 0;
    refresh();
  }

  document.querySelector("#open-message-log").addEventListener("click", () => {
    offset = 0;
    dialog.showModal();
    refresh();
  });

  document.querySelector("#close-message-log").addEventListener("click", () => dialog.close());

  document.querySelector("#message-log-filters").addEventListener("submit", (event) => {
    event.preventDefault();
    resetAndRefresh();
  });

  for (const control of document.querySelectorAll(
    "#log-severity, #log-source, #log-search",
  )) {
    control.addEventListener(control.tagName === "SELECT" ? "change" : "input", resetAndRefresh);
  }

  document.querySelectorAll("[data-log-sort]").forEach((header) => {
    header.querySelector("button").addEventListener("click", () => {
      const column = header.dataset.logSort;
      if (sort.column !== column) {
        sort = { column, direction: 1 };
      } else if (sort.direction === 0) {
        sort.direction = 1;
      } else if (sort.direction === 1) {
        sort.direction = -1;
      } else {
        sort = { column: null, direction: 0 };
      }

      document.querySelectorAll("[data-log-sort]").forEach((item) => {
        const active = item.dataset.logSort === sort.column;
        const ariaSort = !active || sort.direction === 0
          ? "none"
          : sort.direction === 1
            ? "ascending"
            : "descending";
        item.setAttribute("aria-sort", ariaSort);

        const button = item.querySelector("button");
        const label = button.textContent.replace(/[ ▲▼]$/, "");
        button.textContent = active && sort.direction !== 0
          ? `${label} ${sort.direction === 1 ? "▲" : "▼"}`
          : label;
      });

      resetAndRefresh();
    });
  });

  copyPageButton.addEventListener("click", async () => {
    const pageRows = [...rows.querySelectorAll("tr")];
    if (!pageRows.length) return;

    const header = ["Время", "Критичность", "Источник", "Сообщение"].join("\t");
    const content = [
      header,
      ...pageRows.map((row) =>
        [...row.cells].map((cell) => cell.textContent.trim()).join("\t"),
      ),
    ].join("\n");

    try {
      await copyTextToClipboard(content);
      copyPageButton.textContent = "Скопировано";
      window.setTimeout(() => {
        copyPageButton.textContent = "Скопировать страницу";
      }, 1500);
    } catch (error) {
      count.textContent = `Не удалось скопировать страницу: ${error.message}`;
    }
  });

  pageSizeSelect.addEventListener("change", (event) => {
    pageSize = Number(event.currentTarget.value) || 100;
    resetAndRefresh();
  });

  document.querySelector("#message-log-prev").addEventListener("click", () => {
    offset = Math.max(0, offset - pageSize);
    refresh();
  });

  document.querySelector("#message-log-next").addEventListener("click", () => {
    offset += pageSize;
    refresh();
  });

  return {
    refresh,
    refreshIfOpen() {
      if (dialog.open) refresh();
    },
  };
}
