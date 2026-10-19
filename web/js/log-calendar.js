import { localDateKey } from "./log-calendar-utils.js";

export function renderLogCalendarGrid({ month, range, grid, monthLabel }) {
  const year = month.getFullYear();
  const monthIndex = month.getMonth();
  monthLabel.textContent = month.toLocaleDateString("ru-RU", {
    month: "long",
    year: "numeric",
  });

  const weekdays = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];
  grid.replaceChildren();

  for (const weekday of weekdays) {
    const label = document.createElement("span");
    label.className = "log-calendar-weekday";
    label.textContent = weekday;
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
    day.setAttribute("aria-label", date.toLocaleDateString("ru-RU", {
      weekday: "long",
      year: "numeric",
      month: "long",
      day: "numeric",
    }));

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
