/*
 * Keeps the POSITION panel heading controls glued to the coordinate columns.
 *
 * Tasks covered:
 *   • T07 — "on horizontal resize of the Position panel the WPOS and MPOS
 *     buttons must stay right-aligned with their coordinate columns";
 *   • T06 — "the coordinate-system select must be as wide as the Set X Zero /
 *     Home X Zero button pair".
 *
 * Why JS instead of pure CSS: the heading lives in `.panel-heading` (padding
 * 0 7px) while the axis rows live in `.panel-body` (fluid `--panel-pad`
 * padding), so the two grids have different content boxes and their tracks
 * drift apart as soon as the panel is resized. CSS subgrid cannot bridge
 * them without restructuring the panel DOM. Instead this module measures the
 * REAL, rendered geometry of the first axis row and positions each heading
 * control over its corresponding grid track: WPOS over work coordinates,
 * MPOS over machine coordinates, and the coordinate-system selector over the
 * zero/home action pair. Their right edges therefore remain aligned when the
 * panel is resized; the row's 8px grid gap matches the doubled 4px heading gap.
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
  // The coordinate-system control must remain wide enough to show G54–G59.3.
  const MIN_SELECT_WIDTH = 40;
  // Keep the heading controls separated by the doubled 8px grid gap.
  const GROUP_GAP = 8;

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
   * Picks the child element of a row matching `selector`.
   *
   * Row markup is fixed by position-axis-view.js:
   *   [0] label | [1] work value | [2] machine value | [3] zero | [4] home
   * The zero/home buttons are located by their data attributes (they are
   * absent — replaced by <span> placeholders — on unsupported axes, so the
   * caller must handle null).
   *
   * @param {HTMLElement} row - Reference axis row.
   * @param {string} selector - Child selector to look for.
   * @returns {HTMLElement|null}
   */
  function childOf(row, selector) {
    return row.querySelector(selector);
  }

  /**
   * Places one heading control at an exact viewport position.
   *
   * Coordinates are converted from viewport space (getBoundingClientRect) to
   * the heading's own offset-parent space, so the absolute placement is
   * correct in both LTR and RTL layouts without special casing. The caller
   * supplies the width measured from the corresponding live row track.
   *
   * @param {HTMLElement} control - The heading control to move.
   * @param {number} leftViewport - Left edge of the control, viewport px.
   * @param {number} width - Final control width, viewport px.
   * @param {number} headingTop - Top of the heading content box, viewport px.
   * @param {number} headingHeight - Height available for centering, viewport px.
   * @returns {void}
   */
  function place(control, leftViewport, width, headingTop, headingHeight) {
    const origin = heading.getBoundingClientRect();
    const controlHeight = control.offsetHeight || 26;
    control.style.left = `${Math.round(leftViewport - origin.left)}px`;
    control.style.width = `${Math.round(width)}px`;
    control.style.top = `${Math.round(headingTop - origin.top + (headingHeight - controlHeight) / 2)}px`;
  }

  /**
   * Measures the reference row and aligns each heading control to its matching
   * coordinate/action grid track, with a shared-group fallback for hidden columns.
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
      workToggle.classList.remove("position-toggle-compact");
      machineToggle.classList.remove("position-toggle-compact");
      workToggle.style.left = workToggle.style.top = workToggle.style.width = "";
      machineToggle.style.left = machineToggle.style.top = machineToggle.style.width = "";
      coordinateSelect.style.left = coordinateSelect.style.top = coordinateSelect.style.width = "";
      return;
    }

    heading.classList.add(ALIGNED_CLASS);

    const rowRect = row.getBoundingClientRect();
    const headingRect = heading.getBoundingClientRect();
    const workValue = childOf(row, ".axis-value.work");
    const machineValue = childOf(row, ".axis-value.machine");
    const workRect = workValue?.getBoundingClientRect();
    const machineRect = machineValue?.getBoundingClientRect();

    const zeroButton = childOf(row, "[data-zero-axis]");
    const homeButton = childOf(row, "[data-home-axis]");

    // Selector anchor: the combined zero/home button pair. Its 8px grid gap
    // matches the doubled heading-control spacing.
    let groupRight = rowRect.right;
    let pairWidth = MIN_SELECT_WIDTH;
    if (zeroButton && homeButton) {
      const zeroRect = zeroButton.getBoundingClientRect();
      const homeRect = homeButton.getBoundingClientRect();
      const pairLeft = Math.min(zeroRect.left, homeRect.left);
      groupRight = Math.max(zeroRect.right, homeRect.right);
      pairWidth = Math.max(MIN_SELECT_WIDTH, Math.round(groupRight - pairLeft));
    }

    const workVisible = Boolean(workRect && workRect.width > 0);
    const machineVisible = Boolean(machineRect && machineRect.width > 0);
    const contentLeft =
      headingRect.left + parseFloat(getComputedStyle(heading).paddingLeft || "0");
    const selectLeft = groupRight - pairWidth;
    const toggleAvailable = Math.max(
      1,
      selectLeft - contentLeft - GROUP_GAP * 2,
    );
    const toggleWidthLimit = Math.max(1, Math.min(pairWidth, Math.floor(toggleAvailable / 2)));

    /** Bounds one compact heading toggle to its measured coordinate track. */
    function toggleGeometry(trackRect) {
      const right = trackRect.right;
      const width = Math.max(1, Math.min(
        toggleWidthLimit,
        trackRect.width,
        right - contentLeft,
      ));
      return { left: right - width, width };
    }

    let workGeometry;
    let machineGeometry;
    if (workVisible && machineVisible) {
      workGeometry = toggleGeometry(workRect);
      machineGeometry = toggleGeometry(machineRect);
    } else if (workVisible) {
      // The WPOS control stays pinned to the visible work-coordinate edge even
      // when the MPOS value column is collapsed out of the grid.
      workGeometry = toggleGeometry(workRect);
      const machineWidth = Math.max(1, Math.min(
        toggleWidthLimit,
        workGeometry.left - contentLeft - GROUP_GAP,
      ));
      machineGeometry = {
        left: workGeometry.left - GROUP_GAP - machineWidth,
        width: machineWidth,
      };
    } else if (machineVisible) {
      machineGeometry = toggleGeometry(machineRect);
      const workWidth = Math.max(1, Math.min(
        toggleWidthLimit,
        machineGeometry.left - contentLeft - GROUP_GAP,
      ));
      workGeometry = {
        left: machineGeometry.left - GROUP_GAP - workWidth,
        width: workWidth,
      };
    } else {
      const width = toggleWidthLimit;
      machineGeometry = { left: selectLeft - GROUP_GAP - width, width };
      workGeometry = {
        left: machineGeometry.left - GROUP_GAP - width,
        width,
      };
    }

    for (const [control, geometry] of [
      [workToggle, workGeometry],
      [machineToggle, machineGeometry],
    ]) {
      control.classList.toggle("position-toggle-compact", geometry.width < 40);
      place(
        control,
        geometry.left,
        geometry.width,
        headingRect.top,
        headingRect.height,
      );
    }
    place(
      coordinateSelect,
      selectLeft,
      pairWidth,
      headingRect.top,
      headingRect.height,
    );
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
