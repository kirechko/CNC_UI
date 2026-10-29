/*
 * Shrinks the JOG dial instead of showing a scrollbar when the JOG panel is
 * squeezed vertically, and instead of clipping it when the panel is squeezed
 * horizontally.
 *
 * Problem: `.jog-panel > .panel-body` is a scroll container (`overflow-y: auto`).
 * When the row splitter of the left column makes the JOG panel short, the dial
 * keeps its CSS size (`--dial-size`) and the panel-body starts scrolling —
 * the user sees an ugly bottom scrollbar next to the dial.
 *
 * Product decision: while the panel presses on the dial, the DIAL shrinks
 * instead of scrolling, but never below half of its natural (viewport-based)
 * size. If even the halved dial does not fit, the native scrollbar remains as
 * the last-resort fallback.
 *
 * The same rule holds for the width. The dial is the leftmost item of
 * `.jog-dial-layout`, so when the column splitter narrows the panel and the
 * row (dial + Z column + rotary columns + gaps) no longer fits, the centered
 * row is clipped at the LEFT panel edge — exactly the dial. The fit therefore
 * also computes a horizontal cap (body content width minus the side items and
 * gaps) and reduces `--dial-size` so the whole row always fits: the dial is
 * never cut off, it just gets smaller (down to the same half-size floor).
 *
 * How the natural size is measured: `--dial-size` is defined on `:root` as a
 * fluid `clamp()`, and this module writes an override of the same variable on
 * `.jog-panel` only. Reading the variable back from the panel would return the
 * override, not the natural value, so a tiny invisible probe element is
 * attached to `document.body` — the body never carries the override, therefore
 * `width: var(--dial-size)` on the probe resolves to the natural root value.
 *
 * The fit works with measured content height, not assumptions about the DOM.
 * `scrollHeight` cannot be used for the growth direction: by spec it never
 * drops below `clientHeight`, so free slack is invisible through it. Instead
 * the bottom of the last visible block of the panel body is measured:
 *   contentHeight = max(childRect.bottom) - bodyTop + paddingBottom + scrollTop
 *   overflow      = contentHeight - clientHeight   (positive: does not fit)
 *   slack         = -overflow                      (positive: free space below)
 * The dial row is the tallest block of the panel body (the Z and rotary columns
 * stay at 92% of the dial height — jog-z-fit.js re-fits them automatically
 * because it observes the canvas), so reducing the dial by X reduces the
 * content height by about X, which makes the simple target formula converge
 * in one or two passes:
 *   shrink: target = currentDial - overflow   (clamped to [natural/2, natural])
 *   grow:   target = currentDial + slack      (clamped, override removed at natural)
 *
 * Re-redraw: the dial is a canvas whose geometry is radius-relative; after the
 * CSS size changes, the host page must repaint it. That is done through the
 * `onDialResize` callback (main.js passes drawJogDial).
 *
 * Modes: when the dial panel is hidden (keyboard-button mode), the override is
 * cleared and no measuring happens; the keys layout is compact by itself.
 */

/** Smallest allowed share of the natural dial size (user rule: halve at most). */
const MIN_RATIO = 0.5;

/**
 * Starts the adaptive dial sizing.
 *
 * @param {Object} options - Wiring of the fit.
 * @param {HTMLElement} options.panel - The `.jog-panel` element; the `--dial-size`
 *   override is written exclusively on this element so nothing else in the UI
 *   is affected.
 * @param {HTMLElement} options.body - `.panel-body` of the JOG panel: the scroll
 *   container whose overflow decides whether the dial must shrink.
 * @param {HTMLElement} options.dialPanel - `#jog-dial-panel`: hidden in keys mode,
 *   visible in dial mode; also observed so a mode switch re-runs the fit.
 * @param {HTMLElement} options.dialCanvas - The dial `<canvas>`: its CSS height is
 *   the current effective dial size.
 * @param {() => void} options.onDialResize - Called after the size changed so the
 *   host can repaint the canvas (drawJogDial).
 * @returns {{ refresh: () => void, destroy: () => void }} Handle for manual re-fit and teardown.
 */
export function initJogDialFit({ panel, body, dialPanel, dialCanvas, onDialResize }) {
  // Invisible probe living on <body> where `--dial-size` is NOT overridden:
  // its resolved width equals the natural, viewport-based dial diameter.
  const probe = document.createElement("div");
  probe.setAttribute("aria-hidden", "true");
  probe.style.position = "absolute";
  probe.style.top = "-10px";
  probe.style.left = "-10px";
  probe.style.width = "var(--dial-size)";
  probe.style.height = "1px";
  probe.style.visibility = "hidden";
  probe.style.pointerEvents = "none";
  document.body.appendChild(probe);

  /** True while an inline `--dial-size` override is applied to the panel. */
  let overrideActive = false;

  /**
   * Natural dial diameter in pixels, resolved through the body probe.
   * @returns {number} Diameter in CSS pixels (0 when the probe cannot measure).
   */
  function naturalSize() {
    return probe.getBoundingClientRect().width;
  }

  /**
   * Removes the inline override so `:root`'s fluid `clamp()` applies again.
   * @param {boolean} notify - Whether the host should repaint the canvas.
   * @returns {void}
   */
  function clearOverride(notify) {
    if (!overrideActive) return;
    overrideActive = false;
    panel.style.removeProperty("--dial-size");
    if (notify) onDialResize();
  }

  /**
   * Writes a new dial diameter onto the panel and repaints via the callback.
   * @param {number} size - Diameter in CSS pixels.
   * @returns {void}
   */
  function applySize(size) {
    overrideActive = true;
    panel.style.setProperty("--dial-size", `${Math.round(size * 10) / 10}px`);
    onDialResize();
  }

  /**
   * Signed overflow of the panel body content: positive when the content does
   * not fit (a scrollbar would appear), negative when free space is left below
   * the last visible block.
   *
   * `scrollHeight` is NOT usable here: per spec it never goes below
   * `clientHeight`, so "free space" would always read as 0 and the dial could
   * shrink but never grow back. Instead the bottom edge of the last visible
   * child plus the body's bottom padding is compared with the client box.
   * @returns {number} Overflow in CSS pixels.
   */
  function contentOverflow() {
    const bodyRect = body.getBoundingClientRect();
    const paddingBottom = parseFloat(getComputedStyle(body).paddingBottom) || 0;
    let bottom = bodyRect.top;
    for (const child of body.children) {
      if (child.hidden) continue;
      const rect = child.getBoundingClientRect();
      if (rect.height <= 0) continue;
      bottom = Math.max(bottom, rect.bottom);
    }
    // `scrollTop` keeps the measure correct even if the user scrolled the body.
    return bottom - bodyRect.top + paddingBottom + body.scrollTop - body.clientHeight;
  }

  /**
   * Largest dial diameter that still fits the dial row horizontally: the
   * content width of the panel body minus the side items of `.jog-dial-layout`
   * (the Z column and, when shown, the rotary columns) and the flex gaps in
   * front of them. Returns Infinity when nothing can be measured, which the
   * fit treats as "no horizontal constraint".
   *
   * Only widths that do not depend on `--dial-size` are summed (the side
   * columns are sized by their buttons' min-width), so the cap is stable and
   * the rAF loop cannot oscillate around it.
   * @returns {number} Diameter cap in CSS pixels.
   */
  function horizontalCap() {
    const layout = dialPanel.querySelector(".jog-dial-layout");
    if (!layout) return Number.POSITIVE_INFINITY;
    const bodyStyle = getComputedStyle(body);
    // `clientWidth` includes the padding, so subtract it to get the content box.
    const available =
      body.clientWidth -
      (parseFloat(bodyStyle.paddingLeft) || 0) -
      (parseFloat(bodyStyle.paddingRight) || 0);
    if (!(available > 0)) return Number.POSITIVE_INFINITY;
    const gap = parseFloat(getComputedStyle(layout).columnGap) || 0;
    const control = dialCanvas.closest(".jog-dial-control");
    let sideItems = 0;
    let othersWidth = 0;
    for (const child of layout.children) {
      // The dial control itself is the item being sized — skip it.
      if (child === control) continue;
      const rect = child.getBoundingClientRect();
      // Hidden columns (keys mode, continuous Z, no rotary axes) occupy no space.
      if (child.hidden || rect.width <= 0) continue;
      othersWidth += rect.width;
      sideItems += 1;
    }
    // Every visible side item sits after the dial, so it adds one gap before it.
    return available - othersWidth - gap * sideItems;
  }

  /**
   * One measuring pass. Two constraints share the single dial diameter:
   *  - horizontal: the row must fit the body content width, otherwise the
   *    centered row overflows and the dial — the leftmost item — is clipped at
   *    the panel edge;
   *  - vertical: shrink the dial when the body content overflows, grow it back
   *    when free space appears below the last block.
   * Both clamp the result to [natural/2, natural]. A <0.5px difference is
   * treated as "already exact", which makes the rAF/observer feedback loop
   * with jog-z-fit.js converge instead of oscillating.
   * @returns {void}
   */
  function fit() {
    scheduled = 0;
    // Keys mode (or a collapsed panel): no dial to size — keep the natural value.
    if (dialPanel.hidden) {
      clearOverride(false);
      return;
    }

    const natural = naturalSize();
    if (!(natural > 0)) return;
    const floor = natural * MIN_RATIO;
    // Combined horizontal cap; Infinity (nothing measurable) keeps natural.
    const cap = Math.max(floor, Math.min(natural, horizontalCap()));

    const current = dialCanvas.getBoundingClientRect().height;
    const overflow = contentOverflow();

    // Horizontal constraint first: an over-wide row clips the dial's left edge
    // immediately, while vertical overflow only adds a scrollbar.
    if (cap < current - 0.5) {
      applySize(cap);
      return;
    }

    if (overflow > 0.5) {
      // Shrink by exactly the overflow, but never below the floor or the cap.
      const target = Math.max(floor, Math.min(cap, current - overflow));
      if (target < current - 0.5) applySize(target);
      return;
    }

    if (overrideActive && overflow < -0.5) {
      // Slack available: grow toward the allowed maximum (cap or natural);
      // remove the override entirely once the dial is back to its fluid value.
      const target = Math.min(cap, current - overflow);
      if (cap >= natural - 0.5 && target >= natural - 0.5) clearOverride(true);
      else if (target > current + 0.5) applySize(target);
    }
  }

  // Coalesce the burst of ResizeObserver notifications during a splitter drag
  // into a single measured pass per animation frame.
  let scheduled = 0;
  function scheduleFit() {
    if (scheduled) return;
    scheduled = requestAnimationFrame(() => {
      scheduled = 0;
      fit();
    });
  }

  // Observe only the two boxes that the user can change: the panel body (the
  // row splitter / window resize alter its clientHeight) and the dial panel
  // (a mode switch shows or hides it). The canvas is deliberately NOT observed
  // — its size changes are our own output and would re-trigger the fit loop;
  // jog-z-fit.js watches the canvas and adapts the Z/rotary columns itself.
  const observer = new ResizeObserver(scheduleFit);
  observer.observe(body);
  observer.observe(dialPanel);
  // Viewport resizes also move the natural `clamp()` value; re-fit then.
  window.addEventListener("resize", scheduleFit);

  // Initial pass for the stored splitter geometry restored at load time.
  scheduleFit();

  return {
    refresh: scheduleFit,
    destroy: () => {
      if (scheduled) cancelAnimationFrame(scheduled);
      observer.disconnect();
      window.removeEventListener("resize", scheduleFit);
      probe.remove();
    },
  };
}
