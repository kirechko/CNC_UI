/*
 * Jog button columns for the rotary axes A, B and C.
 *
 * The columns sit to the right of the Z column in the dial JOG panel. Each column
 * contains the same kind of controls as Z: a set of incremental step buttons
 * (data-distance fixed in markup) and a pair of continuous buttons. Which of the
 * two groups is visible is switched by the motion mode in jog-mode.js. Each column
 * is shown only when its axis is present in the machine configuration.
 *
 * The column header is plain text (task: "make the A–C header text, not a button,
 * like Z"), so it carries no click handler and no unit toggle. The unit shown in
 * it is derived from the machine configuration: axes reported with angular units
 * display "°", linear axes display "mm" — matching how the axis is actually
 * commanded. Movement itself is sent through the regular jog buttons
 * (data-axis / data-direction / data-distance), so the feed for A–C comes from the
 * rotary speed set on the server.
 */

import { t } from "./i18n.js";

/** Rotary axes that may have a jog column, in display order. */
const ROTARY_AXES = ["A", "B", "C"];

/** Unit label shown for each unit key. */
const UNIT_LABELS = { deg: "°", mm: "mm" };

/**
 * Maps a machine-configuration axis entry to the unit key used by UNIT_LABELS.
 *
 * `AxisUnits` is serialized by the backend as the enum name (`"Angular"` /
 * `"Linear"`); the comparison is case-insensitive so a future snake_case change
 * does not silently fall back to millimetres for rotary axes.
 *
 * @param {object} axisEntry - One entry of status.configuration.axes.
 * @returns {"deg" | "mm"} Unit key for the column header.
 */
function unitKeyOf(axisEntry) {
  const units = String(axisEntry?.units ?? "").toLowerCase();
  return units.includes("angular") ? "deg" : "mm";
}

/**
 * Initializes the rotary jog columns (A, B, C) inside the given container.
 *
 * @param {Object} options - Configuration of the columns.
 * @param {HTMLElement} options.container - Element that holds the columns (#jog-rotary-columns).
 * @param {HTMLElement} [options.keysContainer] - Optional keypad row container
 *   (#jog-keys-rotary) with the A–C +/- button columns of the button JOG mode
 *   (task: the keypad must gain A–C buttons when those axes exist); its columns
 *   are keyed by [data-rotary-key] and shown/hidden by the same render().
 * @param {HTMLElement} [options.stepSelectsContainer] - Optional container of
 *   the per-axis step selects (#jog-rotary-step-selects), shown/hidden with the
 *   same machine configuration; selects are keyed by [data-rotary-step-select].
 * @returns {{ render: Function, refresh: Function }} render() applies the machine configuration; refresh() updates labels.
 */
export function initJogRotaryColumns({ container, keysContainer, stepSelectsContainer }) {
  // Current unit of each column; degrees are the default until the machine
  // configuration arrives (then `render()` recomputes it from the axis units).
  const units = new Map(ROTARY_AXES.map((axis) => [axis, "deg"]));
  // Column elements keyed by axis letter.
  const columns = new Map();
  container.querySelectorAll("[data-rotary-column]").forEach((column) => {
    columns.set(column.dataset.rotaryColumn, column);
  });

  // Keypad button columns and step selects of the same axes (task: keypad
  // A–C controls). Both optional; absent elements are simply skipped.
  const keyColumns = new Map();
  keysContainer?.querySelectorAll("[data-rotary-key]").forEach((column) => {
    keyColumns.set(column.dataset.rotaryKey, column);
  });
  const stepSelects = new Map();
  stepSelectsContainer?.querySelectorAll("[data-rotary-step-select]").forEach((block) => {
    stepSelects.set(block.dataset.rotaryStepSelect, block);
  });

  /**
   * (Re)applies the header unit text and localized tooltips / ARIA labels.
   *
   * Step buttons show only the sign and the fixed distance; the unit lives in
   * the plain-text header. Because the axis letter is a runtime parameter,
   * these strings cannot use static data-i18n-* attributes and are driven here
   * with t(key, params) instead. The function is called at init, whenever the
   * machine configuration changes the unit of an axis, on every status poll
   * (via render()) and on the cnc-locale-changed event.
   */
  function refresh() {
    for (const [axis, column] of columns) {
      const unit = UNIT_LABELS[units.get(axis)];

      // Plain text header (task: "text, not a button, like the Z column"):
      // decorative for assistive tech, because the column itself already
      // carries an aria-label describing the axis movement.
      const head = column.querySelector(".jog-rotary-head");
      if (head) {
        head.textContent = `${axis} · ${unit}`;
        head.removeAttribute("title");
      }

      // Container grouping the incremental step buttons of this axis.
      column
        .querySelector(".jog-rotary-steps")
        ?.setAttribute("aria-label", t("html.jog.rotary.stepsAria", { axis }));

      // Keypad +/− buttons and the step select caption/tooltip for this axis:
      // localized here because the axis letter is a runtime parameter.
      const keyColumn = keyColumns.get(axis);
      if (keyColumn) {
        keyColumn.setAttribute("aria-label", t("html.jog.rotary.keysAria", { axis }));
        keyColumn.querySelectorAll("button").forEach((button) => {
          const positive = Number(button.dataset.direction) > 0;
          const label = t(
            positive ? "html.jog.rotary.keyPlusTitle" : "html.jog.rotary.keyMinusTitle",
            { axis },
          );
          button.title = label;
          button.setAttribute("aria-label", label);
        });
      }
      const selectBlock = stepSelects.get(axis);
      if (selectBlock) {
        const label = selectBlock.querySelector("label");
        const select = selectBlock.querySelector("select");
        if (label) label.textContent = t("html.jog.rotary.stepLabel", { axis });
        const selectTitle = t("html.jog.rotary.stepTitle", { axis });
        if (select) {
          select.title = selectTitle;
          select.setAttribute("aria-label", selectTitle);
        }
      }

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
    const axisEntries = Array.isArray(configuration?.axes) ? configuration.axes : [];
    const presentAxes = new Set(axisEntries.map((item) => String(item.axis).toUpperCase()));

    // Derive each column's unit from the configuration instead of a toggle.
    for (const entry of axisEntries) {
      const axis = String(entry.axis).toUpperCase();
      if (columns.has(axis)) units.set(axis, unitKeyOf(entry));
    }

    let anyVisible = false;
    for (const [axis, column] of columns) {
      column.hidden = !presentAxes.has(axis);
      if (!column.hidden) anyVisible = true;
    }
    container.hidden = !anyVisible;

    // The keypad mirrors the same axis presence: its rotary columns and the
    // per-axis step selects appear exactly when the dial columns do (task).
    for (const [axis, keyColumn] of keyColumns) {
      keyColumn.hidden = !presentAxes.has(axis);
    }
    if (keysContainer) keysContainer.hidden = keysContainer.querySelector("[data-rotary-key]:not([hidden])") === null;
    for (const [axis, block] of stepSelects) {
      block.hidden = !presentAxes.has(axis);
    }
    if (stepSelectsContainer) stepSelectsContainer.hidden = stepSelectsContainer.querySelector("[data-rotary-step-select]:not([hidden])") === null;

    refresh();
  }

  // Localize the static markup of all columns right away, so tooltips are
  // translated even before the first machine configuration arrives.
  refresh();

  return { render, refresh };
}
