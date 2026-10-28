import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

export function initCommandConsole({ logEvent, logError, refreshStatus, isConnected }) {
  const input = document.querySelector("#machine-command-input");
  const form = document.querySelector("#machine-command-form");
  const runButton = document.querySelector("#machine-command-run");
  const inputWrap = document.querySelector("#machine-command-input-wrap");
  const searchIndicator = document.querySelector("#command-search-indicator");
  const historyDialog = document.querySelector("#command-history-dialog");
  const historySearch = document.querySelector("#command-history-search");
  const historyResult = document.querySelector("#command-history-result");
  const historyUseButton = document.querySelector("#command-history-use");

  let history = [];
  let historyCursor = 0;
  let historyDraft = "";
  let lastSearch = "";
  let reverseSearch = null;
  let historyMatchIndex = -1;

  function addToHistory(command) {
    if (history.at(-1) !== command) history.push(command);
    history = history.slice(-500);
    historyCursor = history.length;
    historyDraft = "";
  }

  async function loadHistory() {
    try {
      const result = await invokeOrFetch(
        "get_command_history",
        null,
        "/api/command-history",
      );
      history = Array.isArray(result)
        ? result.filter((command) => typeof command === "string")
        : [];
      historyCursor = history.length;
    } catch (error) {
      console.warn(t("command.history.loadError"), error);
    }
  }

  function moveHistory(direction) {
    if (!history.length) return;

    if (direction < 0) {
      if (historyCursor === history.length) historyDraft = input.value;
      historyCursor = Math.max(0, historyCursor - 1);
      input.value = history[historyCursor] || "";
    } else if (historyCursor < history.length) {
      historyCursor += 1;
      input.value = historyCursor === history.length
        ? historyDraft
        : history[historyCursor];
    }

    input.setSelectionRange(input.value.length, input.value.length);
  }

  function findMatch(query, startIndex = history.length - 1) {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    if (!normalizedQuery) return -1;

    for (let index = Math.min(startIndex, history.length - 1); index >= 0; index--) {
      if (history[index].toLocaleLowerCase().includes(normalizedQuery)) return index;
    }
    return -1;
  }

  function updateDialogSearch(startIndex = history.length - 1) {
    lastSearch = historySearch.value;
    historyMatchIndex = findMatch(lastSearch, startIndex);
    historyUseButton.disabled = historyMatchIndex < 0;
    historyResult.textContent = historyMatchIndex >= 0
      ? t("command.history.match", {
        index: history.length - historyMatchIndex,
        total: history.length,
        command: history[historyMatchIndex],
      })
      : lastSearch.trim()
        ? t("command.history.noMatches")
        : t("command.history.prompt");
  }

  function openDialogSearch() {
    historySearch.value = lastSearch;
    updateDialogSearch();
    historyDialog.showModal();
    historySearch.focus();
    historySearch.select();
  }

  historySearch.addEventListener("input", () => updateDialogSearch());
  historySearch.addEventListener("keydown", (event) => {
    if (!event.ctrlKey || event.key.toLowerCase() !== "r") return;
    event.preventDefault();
    event.stopPropagation();
    updateDialogSearch(historyMatchIndex >= 0 ? historyMatchIndex - 1 : history.length - 1);
  });

  document.querySelector("#command-history-form").addEventListener("submit", (event) => {
    event.preventDefault();
    if (historyMatchIndex < 0) return;

    historyCursor = historyMatchIndex;
    historyDraft = "";
    input.value = history[historyMatchIndex];
    input.setSelectionRange(input.value.length, input.value.length);
    historyDialog.close();
    input.focus();
  });

  document.querySelector("#command-history-cancel").addEventListener("click", () => {
    historyDialog.close();
    input.focus();
  });

  function renderReverseSearch(matchFrom = history.length - 1) {
    if (!reverseSearch) return;

    const query = reverseSearch.query;
    lastSearch = query;
    searchIndicator.textContent = t("command.search.indicator", { query });
    historyMatchIndex = findMatch(query, matchFrom);

    if (historyMatchIndex < 0) {
      input.value = query;
      input.setSelectionRange(query.length, query.length);
      return;
    }

    const match = history[historyMatchIndex];
    input.value = match;
    const matchStart = match.toLocaleLowerCase().indexOf(query.toLocaleLowerCase());
    input.setSelectionRange(matchStart, matchStart + query.length);
  }

  function finishReverseSearch(accept) {
    if (!reverseSearch) return;

    const search = reverseSearch;
    reverseSearch = null;
    inputWrap.classList.remove("searching");
    searchIndicator.textContent = "";

    if (accept && historyMatchIndex >= 0) {
      input.value = history[historyMatchIndex];
      historyCursor = historyMatchIndex;
      historyDraft = "";
    } else {
      input.value = search.draft;
      historyCursor = history.length;
      historyDraft = search.draft;
    }

    input.setSelectionRange(input.value.length, input.value.length);
  }

  input.addEventListener("keydown", (event) => {
    if (event.ctrlKey && ["r", "к"].includes(event.key.toLowerCase())) {
      event.preventDefault();
      if (!reverseSearch) {
        reverseSearch = { query: "", draft: input.value };
        inputWrap.classList.add("searching");
        renderReverseSearch();
      } else {
        renderReverseSearch(
          historyMatchIndex >= 0 ? historyMatchIndex - 1 : history.length - 1,
        );
      }
      return;
    }

    if (reverseSearch) {
      if (event.key === "Escape") {
        event.preventDefault();
        finishReverseSearch(false);
      } else if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        finishReverseSearch(true);
      } else if (event.key === "Backspace") {
        event.preventDefault();
        reverseSearch.query = reverseSearch.query.slice(0, -1);
        renderReverseSearch();
      } else if (
        event.key.length === 1 &&
        !event.ctrlKey &&
        !event.altKey &&
        !event.metaKey
      ) {
        event.preventDefault();
        reverseSearch.query += event.key;
        renderReverseSearch();
      }
      return;
    }

    if (event.key === "ArrowUp") {
      event.preventDefault();
      moveHistory(-1);
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      moveHistory(1);
    } else if (event.key === "Escape") {
      historyCursor = history.length;
      input.value = historyDraft;
    }
  });

  input.addEventListener("blur", () => {
    if (reverseSearch) finishReverseSearch(true);
  });

  input.addEventListener("input", () => {
    if (historyCursor === history.length) historyDraft = input.value;
  });

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (reverseSearch) finishReverseSearch(true);

    const command = input.value.trim();
    if (!command) return;

    addToHistory(command);
    runButton.disabled = true;
    try {
      const output = await invokeOrFetch(
        "execute_machine_command",
        { command },
        "/api/command",
        "POST",
      );
      input.value = "";
      // Example: G0X-22 is a rapid move; the actual motion depends on the
      // controller's current coordinate mode (G90/G91). If the backend
      // returns an object, a template string would show "[object Object]",
      // so such a response is serialized to JSON for the log.
      const formattedOutput = output == null
        ? ""
        : typeof output === "string"
          ? output
          : JSON.stringify(output);
      const result = formattedOutput
        ? t("command.executedWithResponse", { command, response: formattedOutput })
        : t("command.executed", { command });
      await logEvent("info", result);
      await refreshStatus();
    } catch (error) {
      await logError(t("command.failed", { command }), error);
    } finally {
      runButton.disabled = !isConnected();
    }
  });

  return { loadHistory, searchHistory: openDialogSearch };
}
