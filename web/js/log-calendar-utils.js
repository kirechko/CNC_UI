import { t } from "./i18n.js";

export function localDateKey(date) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return `${year}-${month}-${day}`;
}

export function parseLocalDate(key) {
  if (!key) return null;
  const [year, month, day] = key.split("-").map(Number);
  return new Date(year, month - 1, day);
}

export function formatLocalDate(key) {
  return parseLocalDate(key)?.toLocaleDateString() || "";
}

export function logRangeLabel(range) {
  // No start date means the filter covers all dates; the label is localized.
  if (!range.start) return t("html.log.anyDate");
  if (!range.end || range.start === range.end) return formatLocalDate(range.start);
  return `${formatLocalDate(range.start)} — ${formatLocalDate(range.end)}`;
}
