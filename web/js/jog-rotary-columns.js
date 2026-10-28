/*
 * Jog button columns for the rotary axes A, B and C.
 *
 * The columns sit to the right of the Z column in the dial JOG panel. Each column
 * contains the same kind of controls as Z: a set of incremental step buttons
 * (data-distance fixed in markup) and a pair of continuous buttons. Which of the
 * two groups is visible is switched by the motion mode in jog-mode.js. Each column
 * is shown only when its axis is present in the machine configuration. Clicking a
 * column header switches the unit label of that column between degrees and
 * millimetres. Movement itself is sent through the regular jog buttons
 * (data-axis / data-direction / data-distance), so the feed for A–C comes from the
 * rotary speed set on the server.
 */

import { t } from "./i18n.js";

/** Rotary axes that may have a jog column, in display order. */
const ROTARY_AXES = ["A", "B", "C"];

/** Unit label shown for each unit key. */
const UNIT_LABELS = { deg: "°", mm: "mm" };

/**
 * Initializes the rotary jog columns (A, B, C) inside the given container.
 *
 * @param {Object} options - Configuration of the columns.
 * @param {HTMLElement} options.container - Element that holds the columns (#jog-rotary-columns).
 * @returns {{ render: Function, refresh: Function }} render() applies the machine configuration; refresh() updates labels.
 */
export function initJogRotaryColumns({ container }) {
  // Current unit of each column; degrees are the default.
  const units = new Map(ROTARY_AXES.map((axis) => [axis, "deg"]));
  // Column elements keyed by axis letter.
  const columns = new Map();
  container.querySelectorAll("[data-rotary-column]").forEach((column) => {
    columns.set(column.dataset.rotaryColumn, column);
  });

  /**
   * Updates the header of every column from its current unit and (re)applies
   * localized tooltips and ARIA labels. Step buttons show only the sign and
   * the fixed distance; the unit is in the header. Because the axis letter is
   * a runtime parameter, these strings cannot use static data-i18n-*
   * attributes and are driven here with t(key, params) instead. The function
   * is called at init, after every unit toggle, on every status poll (via
   * render()) and on the cnc-locale-changed event.
   */
  function refresh() {
    for (const [axis, column] of columns) {
      const unit = UNIT_LABELS[units.get(axis)];

      // The header toggles the unit of its column between degrees and millimetres.
      const head = column.querySelector(".jog-rotary-head");
      head.textContent = `${axis} · ${unit}`;
      head.title = t("html.jog.rotary.unitTitle", { axis });
      head.setAttribute("aria-label", t("html.jog.rotary.unitAria", { axis }));

      // Container grouping the incremental step buttons of this axis.
      column
        .querySelector(".jog-rotary-steps")
        ?.setAttribute("aria-label", t("html.jog.rotary.stepsAria", { axis }));

      // Each step button carries its own distance in data-distance and direction in data-direction.
      column.querySelectorAll(".jog-rotary-step").forEach((button) => {
        const positive = Number(button.dataset.direction) > 0;
        const sign = positive ? "+" : "−";
        button.textContent = `${sign}${button.dataset.distance}`;
        const label = t("html.jog.rotary.stepTitle", {
          axis,
          direction: t(positive ? "jog.direction.plus" : "jog.direction.minus"),
          distance: button.dataset.distance,
        });
        button.title = label;
        button.setAttribute("aria-label", label);
      });

      // Continuous buttons: holding the button moves the axis until release.
      column.querySelectorAll(".jog-rotary-continuous button").forEach((button) => {
        const direction = t(
          Number(button.dataset.direction) > 0 ? "jog.direction.plus" : "jog.direction.minus",
        );
        button.title = t("html.jog.rotary.holdTitle", { axis, direction });
        button.setAttribute("aria-label", t("html.jog.rotary.holdAria", { axis, direction }));
      });
    }
  }

  /**
   * Shows only the columns whose axes exist in the machine configuration.
   * The whole container is hidden when none of the rotary axes is present.
   *
   * @param {object} configuration - Machine configuration payload (status.configuration).
   */
  function render(configuration) {
    const presentAxes = new Set(
      Array.isArray(configuration?.axes)
        ? configuration.axes.map((item) => String(item.axis).toUpperCase())
        : [],
    );

    let anyVisible = false;
    for (const [axis, column] of columns) {
      column.hidden = !presentAxes.has(axis);
      if (!column.hidden) anyVisible = true;
    }

    container.hidden = !anyVisible;
    refresh();
  }

  // Clicking a column header toggles its unit between degrees and millimetres.
  container.addEventListener("click", (event) => {
    const toggle = event.target.closest("[data-rotary-unit-toggle]");
    if (!toggle) return;

    const axis = toggle.dataset.rotaryUnitToggle;
    units.set(axis, units.get(axis) === "deg" ? "mm" : "deg");
    refresh();
  });

  // Localize the static markup of all columns right away, so tooltips are
  // translated even before the first machine configuration arrives.
  refresh();

  return { render, refresh };
}
