import { logRangeLabel, parseLocalDate } from "./log-calendar-utils.js";
import { renderLogCalendarGrid } from "./log-calendar.js";

const EMPTY_DATE_RANGE = {
  start: null,
  end: null,
  startTime: "00:00",
  endTime: "23:59",
};

export function initLogCalendar({ onChange = () => {} } = {}) {
  const calendar = document.querySelector("#log-calendar");
  const grid = document.querySelector("#log-calendar-grid");
  const monthLabel = document.querySelector("#log-calendar-month");
  const startTime = document.querySelector("#log-calendar-start-time");
  const endTime = document.querySelector("#log-calendar-end-time");
  const pickerButton = document.querySelector("#log-date-picker-button");

  if (!calendar || !grid || !monthLabel || !startTime || !endTime || !pickerButton) {
    return { getRange: () => ({ ...EMPTY_DATE_RANGE }) };
  }

  let selectedRange = { ...EMPTY_DATE_RANGE };
  let draftRange = { ...selectedRange };
  let displayedMonth = new Date();
  let dragging = false;

  function updateTimeControls() {
    const hasDate = Boolean(draftRange.start);
    startTime.value = draftRange.startTime || "00:00";
    endTime.value = draftRange.endTime || "23:59";
    startTime.disabled = !hasDate;
    endTime.disabled = !hasDate;
  }

  function render() {
    renderLogCalendarGrid({
      month: displayedMonth,
      range: draftRange,
      grid,
      monthLabel,
    });
    updateTimeControls();
  }

  function updateSelectionStyles() {
    const start = draftRange.start;
    const end = draftRange.end || start;

    for (const day of grid.querySelectorAll("[data-date]")) {
      const key = day.dataset.date;
      day.classList.toggle("in-range", Boolean(start && end && key >= start && key <= end));
      day.classList.toggle("range-start", key === start);
      day.classList.toggle("range-end", key === draftRange.end);
    }
    updateTimeControls();
  }

  function commitSelection() {
    selectedRange = { ...draftRange };
    pickerButton.textContent = logRangeLabel(selectedRange);
    onChange(selectedRange);
  }

  function selectDraggedDay(key) {
    draftRange = { ...draftRange, start: key, end: key };
    dragging = true;
    updateSelectionStyles();
  }

  function extendDraggedSelection(key) {
    if (!dragging || !draftRange.start) return;

    const anchor = draftRange.start;
    draftRange = anchor <= key
      ? { ...draftRange, start: anchor, end: key }
      : { ...draftRange, start: key, end: anchor };
    updateSelectionStyles();
  }

  pickerButton.addEventListener("click", () => {
    const opening = calendar.hidden;
    calendar.hidden = !opening;
    pickerButton.setAttribute("aria-expanded", String(opening));

    if (opening) {
      draftRange = { ...selectedRange };
      displayedMonth = parseLocalDate(draftRange.start) || new Date();
      displayedMonth.setDate(1);
      render();
    }
  });

  document.querySelector("#log-calendar-prev")?.addEventListener("click", () => {
    displayedMonth = new Date(
      displayedMonth.getFullYear(),
      displayedMonth.getMonth() - 1,
      1,
    );
    render();
  });

  document.querySelector("#log-calendar-next")?.addEventListener("click", () => {
    displayedMonth = new Date(
      displayedMonth.getFullYear(),
      displayedMonth.getMonth() + 1,
      1,
    );
    render();
  });

  grid.addEventListener("pointerdown", (event) => {
    const day = event.target.closest("[data-date]");
    if (!day) return;

    event.preventDefault();
    selectDraggedDay(day.dataset.date);
  });

  grid.addEventListener("pointerover", (event) => {
    const day = event.target.closest("[data-date]");
    if (day) extendDraggedSelection(day.dataset.date);
  });

  document.addEventListener("pointerup", () => {
    if (!dragging) return;

    dragging = false;
    commitSelection();
  });

  grid.addEventListener("click", (event) => {
    const day = event.target.closest("[data-date]");
    if (!day || event.detail !== 0) return;

    const key = day.dataset.date;
    if (!draftRange.start || draftRange.end) {
      draftRange = { ...draftRange, start: key, end: null };
    } else {
      const anchor = draftRange.start;
      draftRange = anchor <= key
        ? { ...draftRange, start: anchor, end: key }
        : { ...draftRange, start: key, end: anchor };
    }

    updateSelectionStyles();
    commitSelection();
  });

  startTime.addEventListener("input", () => {
    draftRange.startTime = startTime.value || "00:00";
    updateTimeControls();
  });

  endTime.addEventListener("input", () => {
    draftRange.endTime = endTime.value || "23:59";
    updateTimeControls();
  });

  document.querySelector("#log-calendar-clear")?.addEventListener("click", () => {
    selectedRange = { ...EMPTY_DATE_RANGE };
    draftRange = { ...selectedRange };
    pickerButton.textContent = logRangeLabel(selectedRange);
    render();
    onChange(selectedRange);
  });

  document.querySelector("#log-calendar-done")?.addEventListener("click", () => {
    commitSelection();
    calendar.hidden = true;
    pickerButton.setAttribute("aria-expanded", "false");
  });

  document.addEventListener("pointerdown", (event) => {
    if (!event.target.closest(".log-date-picker")) {
      calendar.hidden = true;
      pickerButton.setAttribute("aria-expanded", "false");
    }
  });

  return {
    getRange() {
      return { ...selectedRange };
    },
  };
}
