import { localDateKey } from "./log-calendar-utils.js";
import { getLocale, t } from "./i18n.js";

/**
 * Renders the month grid of the log period calendar.
 * Date names follow the active UI locale; weekday headers come from the dictionary.
 * @param {{month: Date, range: object, grid: HTMLElement, monthLabel: HTMLElement}} options
 */
export function renderLogCalendarGrid({ month, range, grid, monthLabel }) {
  const year = month.getFullYear();
  const monthIndex = month.getMonth();
  const locale = getLocale();
  monthLabel.textContent = month.toLocaleDateString(locale, {
    month: "long",
    year: "numeric",
  });

  // Weekday headers, Monday-first, matching the ISO week layout used below.
  const weekdayKeys = [
    "calendar.weekday.mon",
    "calendar.weekday.tue",
    "calendar.weekday.wed",
    "calendar.weekday.thu",
    "calendar.weekday.fri",
    "calendar.weekday.sat",
    "calendar.weekday.sun",
  ];
  grid.replaceChildren();

  for (const weekdayKey of weekdayKeys) {
    const label = document.createElement("span");
    label.className = "log-calendar-weekday";
    label.textContent = t(weekdayKey);
    grid.append(label);
  }

  const firstDay = new Date(year, monthIndex, 1);
  const offset = (firstDay.getDay() + 6) % 7;
  const gridStart = new Date(year, monthIndex, 1 - offset);

  for (let index = 0; index < 42; index++) {
    const date = new Date(
      gridStart.getFullYear(),
      gridStart.getMonth(),
      gridStart.getDate() + index,
    );
    const key = localDateKey(date);
    const day = document.createElement("button");
    day.type = "button";
    day.className = "log-calendar-day";
    day.dataset.date = key;
    day.textContent = String(date.getDate());
    // Fully formatted date doubles as the accessible name and the tooltip so
    // hovering a day cell explains which calendar date it represents.
    const dateLabel = date.toLocaleDateString(locale, {
      weekday: "long",
      year: "numeric",
      month: "long",
      day: "numeric",
    });
    day.setAttribute("aria-label", dateLabel);
    day.title = dateLabel;

    if (date.getMonth() !== monthIndex) day.classList.add("outside-month");
    const start = range.start;
    const end = range.end || start;
    if (start && end && key >= start && key <= end) day.classList.add("in-range");
    if (key === start) day.classList.add("range-start");
    if (key === range.end) day.classList.add("range-end");
    if (key === localDateKey(new Date())) day.setAttribute("aria-current", "date");
    grid.append(day);
  }
}
