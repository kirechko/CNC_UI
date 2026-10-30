/*
 * Keeps the POSITION panel heading controls glued to the coordinate columns.
 *
 * Tasks covered:
 *   • T07 — "on horizontal resize of the Position panel the WPOS and MPOS
 *     buttons must stay exactly above their coordinate columns";
 *   • T06 — "the coordinate-system select must be as wide as the Set X Zero /
 *     Home X Zero button pair".
 *
 * Why JS instead of pure CSS: the heading lives in `.panel-heading` (padding
 * 0 7px) while the axis rows live in `.panel-body` (fluid `--panel-pad`
 * padding), so the two grids have different content boxes and their tracks
 * drift apart as soon as the panel is resized. CSS subgrid cannot bridge
 * them without restructuring the panel DOM. Instead this module measures the
 * REAL, rendered geometry of the first axis row and positions the three
 * heading controls in px over the columns they belong to:
 *
 *   WPOS button       -> work-coordinate column of the row
 *   MPOS button       -> machine-coordinate column of the row
 *   coordinate select -> per-axis zero + home button pair (exact width)
 *
 * The measurement re-runs on every event that can change the geometry:
 * window resize, panel splitter drags (ResizeObserver on the panel), the axis
 * rows being re-rendered for another machine (3/4/5/6-axis setups, T20) and
 * WPOS/MPOS visibility toggles (class changes on the rows).
 *
 * Until the first axis row exists — or if this module is not loaded — the
 * static flex layout of position.css stays active, so the header can never
 * break.
 */

/** Class that switches the heading into measured absolute positioning. */
const ALIGNED_CLASS = "position-heading-aligned";

/**
 * Creates the heading aligner.
 *
 * @param {object} options - Elements involved in the alignment.
 * @param {HTMLElement} options.heading - The `.section-heading` inside the
 *   POSITION panel heading (the flex container holding the three controls).
 * @param {HTMLElement} options.workToggle - `#wpos-toggle` button.
 * @param {HTMLElement} options.machineToggle - `#mpos-toggle` button.
 * @param {HTMLElement} options.coordinateSelect - `#coordinate-system` select.
 * @param {HTMLElement} options.axesContainer - `#axis-rows`, the element that
 *   holds the rendered `.axis-row` grids (the geometry reference).
 * @param {HTMLElement} options.panel - The POSITION `<section class="panel">`
 *   (observed with a ResizeObserver so splitter drags re-run the measure).
 * @returns {{ refresh: Function, destroy: Function }} refresh() re-measures
 *   and repositions; destroy() removes the observers and un-aligns the heading.
 */
export function initPositionHeadingAlign({
  heading,
  workToggle,
  machineToggle,
  coordinateSelect,
  axesContainer,
  panel,
}) {
  // Widths the controls must never drop below, so a very narrow panel keeps
  // readable captions instead of collapsing the buttons to a few pixels.
  const MIN_TOGGLE_WIDTH = 34;
  const MIN_SELECT_WIDTH = 40;

  let resizeObserver = null;
  let mutationObserver = null;
  let pendingFrame = 0;

  /**
   * Returns the axis row that defines the column geometry, or null while the
   * rows have not been rendered yet (no machine configuration received).
   *
   * @returns {HTMLElement|null}
   */
  function referenceRow() {
    return axesContainer.querySelector(".axis-row");
  }

  /**
   * Picks the child element of a row that spans the requested column.
   *
   * Row markup is fixed by position-axis-view.js:
   *   [0] label | [1] work value | [2] machine value | [3] zero | [4] home
   * In wpos-only mode the machine value is `display: none`, and in mpos-only
   * mode the work value is, so the visible one must be located by class
   * rather than by index.
   *
   * @param {HTMLElement} row - Reference axis row.
   * @param {string} selector - Child selector to look for.
   * @returns {HTMLElement|null}
   */
  function childOf(row, selector) {
    return row.querySelector(selector);
  }

  /**
   * Positions one heading control over a horizontal span of the row.
   *
   * Coordinates are converted from viewport space (getBoundingClientRect) to
   * the heading's own offset-parent space, so the absolute placement is
   * correct in both LTR and RTL layouts without special casing.
   *
   * @param {HTMLElement} control - The heading control to move.
   * @param {number} leftViewport - Left edge of the target column, viewport px.
   * @param {number} widthViewport - Width of the target column(s), viewport px.
   * @param {number} minWidth - Floor for the control width.
   * @param {number} headingTop - Top of the heading content box, viewport px.
   * @param {number} headingHeight - Height available for centering, viewport px.
   * @returns {void}
   */
  function place(control, leftViewport, widthViewport, minWidth, headingTop, headingHeight) {
    const origin = heading.getBoundingClientRect();
    const controlHeight = control.offsetHeight || 26;
    const width = Math.max(minWidth, Math.round(widthViewport));
    // Keep the control inside the heading row when the column is narrower
    // than the minimum width: center it over the column instead.
    let left = leftViewport - origin.left;
    if (width > widthViewport) left -= (width - widthViewport) / 2;
    control.style.left = `${Math.round(left)}px`;
    control.style.width = `${width}px`;
    control.style.top = `${Math.round(headingTop - origin.top + (headingHeight - controlHeight) / 2)}px`;
  }

  /**
   * Measures the reference row and repositions all three heading controls.
   *
   * Falls back to the static layout (removes ALIGNED_CLASS) when the rows do
   * not exist yet, so nothing is ever positioned over an empty panel.
   *
   * @returns {void}
   */
  function refresh() {
    const row = referenceRow();
    if (!row) {
      heading.classList.remove(ALIGNED_CLASS);
      workToggle.style.left = workToggle.style.top = workToggle.style.width = "";
      machineToggle.style.left = machineToggle.style.top = machineToggle.style.width = "";
      coordinateSelect.style.left = coordinateSelect.style.top = coordinateSelect.style.width = "";
      return;
    }

    heading.classList.add(ALIGNED_CLASS);

    const rowRect = row.getBoundingClientRect();
    const headingRect = heading.getBoundingClientRect();

    const workCell = childOf(row, ".position-column.work");
    const machineCell = childOf(row, ".position-column.machine");
    const zeroButton = childOf(row, "[data-zero-axis]");
    const homeButton = childOf(row, "[data-home-axis]");

    // Work / machine columns: a hidden column (display:none) has a zero-width
    // rect at the position where it would be, so both toggles can always be
    // mapped to a real span of the row.
    const workRect = workCell ? workCell.getBoundingClientRect() : rowRect;
    const machineRect = machineCell ? machineCell.getBoundingClientRect() : rowRect;

    place(
      workToggle,
      workRect.left,
      workRect.width,
      MIN_TOGGLE_WIDTH,
      headingRect.top,
      headingRect.height,
    );
    place(
      machineToggle,
      machineRect.left,
      machineRect.width,
      MIN_TOGGLE_WIDTH,
      headingRect.top,
      headingRect.height,
    );

    // The select must be exactly as wide as the per-axis zero/home pair, i.e.
    // from the left edge of the zero button to the right edge of the home one.
    if (zeroButton && homeButton) {
      const zeroRect = zeroButton.getBoundingClientRect();
      const homeRect = homeButton.getBoundingClientRect();
      const left = Math.min(zeroRect.left, homeRect.left);
      const right = Math.max(zeroRect.right, homeRect.right);
      place(
        coordinateSelect,
        left,
        right - left,
        MIN_SELECT_WIDTH,
        headingRect.top,
        headingRect.height,
      );
    }
  }

  /**
   * Schedules a measurement on the next animation frame.
   *
   * ResizeObserver, MutationObserver and the window resize can all fire in
   * the same frame (splitter drag re-lays out rows AND headings); coalescing
   * them into one frame avoids layout thrash while keeping the pairing exact.
   *
   * @returns {void}
   */
  function scheduleRefresh() {
    if (pendingFrame) return;
    pendingFrame = requestAnimationFrame(() => {
      pendingFrame = 0;
      refresh();
    });
  }

  // Geometry changes: the panel itself (splitter drag, sidebar collapse,
  // window resize) and any layout change of the axis rows.
  if (typeof ResizeObserver !== "undefined") {
    resizeObserver = new ResizeObserver(scheduleRefresh);
    resizeObserver.observe(panel);
    resizeObserver.observe(axesContainer);
  }
  window.addEventListener("resize", scheduleRefresh);

  // Row re-render for another machine, or a WPOS/MPOS display toggle: both
  // rewrite the classes/children of the rows and must re-measure.
  if (typeof MutationObserver !== "undefined") {
    mutationObserver = new MutationObserver(scheduleRefresh);
    mutationObserver.observe(axesContainer, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["class"],
    });
  }

  scheduleRefresh();

  return {
    /** Re-measure immediately (used by callers that just changed the layout). */
    refresh,
    /** Remove all observers and restore the static heading layout. */
    destroy() {
      if (pendingFrame) cancelAnimationFrame(pendingFrame);
      pendingFrame = 0;
      if (resizeObserver) resizeObserver.disconnect();
      if (mutationObserver) mutationObserver.disconnect();
      window.removeEventListener("resize", scheduleRefresh);
      heading.classList.remove(ALIGNED_CLASS);
    },
  };
}
