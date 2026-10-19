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
