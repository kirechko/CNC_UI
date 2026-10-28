/*
 * Fits the stepped Z jog buttons into the free height of the JOG panel.
 *
 * Buttons are hidden one at a time, innermost first, so the outer buttons
 * (+100 / -100) stay visible the longest. The grid rows of the Z column are
 * rebuilt to match the number of visible buttons, so no empty tracks remain.
 * A ResizeObserver re-runs the fit whenever the panel, the dial or the Z
 * column changes size.
 */

/**
 * Hiding order of the stepped Z buttons, innermost first.
 * Keys are "direction:distance" taken from the button data attributes.
 */
const SHRINK_ORDER = ["-1:0.1", "1:0.1", "-1:1", "1:1", "-1:10", "1:10", "-1:100", "1:100"];

/** Minimum usable height of one Z button row in pixels. */
const MIN_ROW_HEIGHT = 20;
/** Height of the "Z" label row; must match the CSS grid-template-rows of the Z column. */
const LABEL_ROW_HEIGHT = 12;
/** Height of the separator row between the upper and lower button groups. */
const GROUP_GAP_HEIGHT = 6;
/** Gap between grid rows; must match the gap of .jog-dial-z-steps in jog.css. */
const ROW_GAP = 2;
/** Maximum share of the dial height used by the Z column. */
const FULL_HEIGHT_RATIO = 0.92;

/**
 * Initializes the adaptive visibility of the stepped Z buttons.
 *
 * @param {Object} options - Configuration of the fit.
 * @param {HTMLElement} options.steps - Container with the stepped Z buttons (#jog-dial-z-steps).
 * @param {HTMLElement} options.layout - Flex row that holds the dial and the Z column (.jog-dial-layout).
 * @param {HTMLElement} options.boundary - Element whose bottom edge limits the available height (.panel-body of the JOG panel).
 * @param {HTMLElement} options.dialCanvas - Dial canvas, used to derive the full Z column height.
 * @param {HTMLElement} [options.rotaryColumns] - Container of the A–C rotary columns (#jog-rotary-columns).
 *   It takes the same height as the Z column so both fit the free panel height. Optional.
 * @returns {{ refresh: Function }} Handle with a refresh() method to request a new fit.
 */
export function initJogZFit({ steps, layout, boundary, dialCanvas, rotaryColumns = null }) {
  // Map of "direction:distance" to the corresponding Z button.
  const buttons = new Map();
  steps.querySelectorAll('button[data-dial-axis="Z"]').forEach((button) => {
    buttons.set(`${button.dataset.direction}:${button.dataset.distance}`, button);
  });
  const gapSpan = steps.querySelector(".jog-dial-gap");
  let scheduled = false;

  // One entry per rotary column (A, B, C): its step container, button map and separator span.
  // Keys use the same "direction:distance" format as the Z buttons so SHRINK_ORDER applies unchanged.
  const rotaryGroups = rotaryColumns
    ? Array.from(rotaryColumns.querySelectorAll(".jog-rotary-steps")).map((container) => {
        const groupButtons = new Map();
        container.querySelectorAll("button.jog-rotary-step").forEach((button) => {
          groupButtons.set(`${button.dataset.direction}:${button.dataset.distance}`, button);
        });
        return {
          container,
          buttons: groupButtons,
          gap: container.querySelector(".jog-dial-gap"),
        };
      })
    : [];

  /**
   * Counts the visible buttons in the upper (+) and lower (-) groups
   * when the first `hiddenCount` entries of SHRINK_ORDER are hidden.
   *
   * @param {number} hiddenCount - Number of innermost buttons that are hidden.
   * @returns {{ upper: number, lower: number }} Visible button counts per group.
   */
  function visibleCounts(hiddenCount) {
    let upper = 0;
    let lower = 0;
    SHRINK_ORDER.forEach((key, index) => {
      if (index < hiddenCount) return;
      if (key.startsWith("1:")) upper += 1;
      else lower += 1;
    });
    return { upper, lower };
  }

  /**
   * Computes the minimum height the Z column needs for the given visible counts.
   *
   * @param {number} upper - Visible buttons in the upper group.
   * @param {number} lower - Visible buttons in the lower group.
   * @returns {number} Required height in pixels.
   */
  function requiredHeight(upper, lower) {
    const bothGroups = upper > 0 && lower > 0;
    // Rows: label, upper buttons, optional separator, lower buttons.
    const rowCount = 1 + upper + lower + (bothGroups ? 1 : 0);
    return (
      LABEL_ROW_HEIGHT +
      (bothGroups ? GROUP_GAP_HEIGHT : 0) +
      (upper + lower) * MIN_ROW_HEIGHT +
      (rowCount - 1) * ROW_GAP
    );
  }

  /**
   * Hides the innermost `hiddenCount` buttons and rebuilds the grid rows
   * so that only the visible buttons occupy tracks.
   *
   * @param {number} hiddenCount - Number of innermost buttons to hide.
   * @returns {{ upper: number, lower: number }} Visible counts after the update.
   */
  function applyHiddenCount(hiddenCount) {
    SHRINK_ORDER.forEach((key, index) => {
      const button = buttons.get(key);
      if (button) button.hidden = index < hiddenCount;
    });

    const counts = visibleCounts(hiddenCount);
    const bothGroups = counts.upper > 0 && counts.lower > 0;
    if (gapSpan) gapSpan.hidden = !bothGroups;

    // Build the explicit track list: label, upper tracks, separator, lower tracks.
    const tracks = [`${LABEL_ROW_HEIGHT}px`];
    if (counts.upper > 0) tracks.push(`repeat(${counts.upper}, minmax(0, 1fr))`);
    if (bothGroups) tracks.push(`${GROUP_GAP_HEIGHT}px`);
    if (counts.lower > 0) tracks.push(`repeat(${counts.lower}, minmax(0, 1fr))`);
    steps.style.gridTemplateRows = tracks.join(" ");

    return counts;
  }

  /**
   * Applies the same hidden count to the step buttons of every rotary column (A–C),
   * so A–C show the same number of buttons as Z. Rotary columns have no label row
   * (their header sits outside the step grid), so their tracks omit LABEL_ROW_HEIGHT.
   *
   * @param {number} hiddenCount - Number of innermost buttons to hide in each column.
   * @returns {void}
   */
  function applyRotaryHiddenCount(hiddenCount) {
    const counts = visibleCounts(hiddenCount);
    const bothGroups = counts.upper > 0 && counts.lower > 0;

    // Track list without the label row: upper tracks, separator, lower tracks.
    const tracks = [];
    if (counts.upper > 0) tracks.push(`repeat(${counts.upper}, minmax(0, 1fr))`);
    if (bothGroups) tracks.push(`${GROUP_GAP_HEIGHT}px`);
    if (counts.lower > 0) tracks.push(`repeat(${counts.lower}, minmax(0, 1fr))`);
    const trackList = tracks.length > 0 ? tracks.join(" ") : "none";

    rotaryGroups.forEach(({ container, buttons, gap }) => {
      SHRINK_ORDER.forEach((key, index) => {
        const button = buttons.get(key);
        if (button) button.hidden = index < hiddenCount;
      });
      if (gap) gap.hidden = !bothGroups;
      container.style.gridTemplateRows = trackList;
    });
  }

  /**
   * Measures the free height of the JOG panel and chooses how many Z buttons
   * to hide so that the Z column fits. Hides buttons innermost first and
   * restores them as soon as the free height allows it.
   *
   * @returns {void}
   */
  function fit() {
    scheduled = false;
    // Skip while the Z column is not rendered (continuous mode or hidden panel).
    if (steps.getClientRects().length === 0) return;

    const available = boundary.getBoundingClientRect().bottom - layout.getBoundingClientRect().top;
    const fullHeight = dialCanvas.getBoundingClientRect().height * FULL_HEIGHT_RATIO;

    // Find the smallest number of hidden buttons that lets the column fit.
    let chosen = SHRINK_ORDER.length;
    for (let hidden = 0; hidden <= SHRINK_ORDER.length; hidden += 1) {
      const counts = visibleCounts(hidden);
      if (requiredHeight(counts.upper, counts.lower) <= available) {
        chosen = hidden;
        break;
      }
    }

    const counts = applyHiddenCount(chosen);
    // A–C follow the same hidden count as Z so every column shows matching buttons.
    applyRotaryHiddenCount(chosen);
    const required = requiredHeight(counts.upper, counts.lower);
    // Use the full column height when there is room, otherwise the available space (never below required).
    const height = Math.min(fullHeight, Math.max(required, available));
    steps.style.height = `${Math.max(0, height)}px`;

    // The rotary columns share the Z column height so they do not overflow the panel.
    if (rotaryColumns && !rotaryColumns.hidden) {
      rotaryColumns.style.height = steps.style.height;
    }
  }

  /**
   * Schedules a fit on the next animation frame, coalescing repeated requests.
   *
   * @returns {void}
   */
  function scheduleFit() {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(fit);
  }

  const observer = new ResizeObserver(scheduleFit);
  observer.observe(boundary);
  observer.observe(layout);
  observer.observe(steps);
  observer.observe(dialCanvas);
  // Showing or hiding a rotary column changes its size, which re-runs the fit.
  if (rotaryColumns) observer.observe(rotaryColumns);
  window.addEventListener("resize", scheduleFit);
  scheduleFit();

  return { refresh: scheduleFit };
}
