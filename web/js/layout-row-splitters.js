// Draggable horizontal splitters inside the left and right columns of the main layout.
// Left column: splitter between the POSITION panel (top) and the JOG panel (bottom).
// Right column: splitter between the MACROS/FILES panel (top) and the Commands panel (bottom).
// Heights are written to CSS custom properties on each column and persisted in localStorage.

// localStorage key holding the last chosen row heights in pixels.
const STORAGE_KEY = "cnc-ui-layout-rows";

// Minimum height in pixels of the panel that is resized by the splitter.
const PANEL_MIN = 120;

// Minimum height in pixels of the panel on the opposite side of the splitter.
const OTHER_MIN = 160;

// Pixels moved per arrow key press when a splitter has keyboard focus.
const KEYBOARD_STEP = 16;

// Media query matching the layout where both columns are visible side by side.
const WIDE_QUERY = "(min-width: 901px)";

/**
 * Restricts a value to the [min, max] range; if max is below min, min wins.
 * @param {number} value Value to clamp.
 * @param {number} min Smallest allowed value.
 * @param {number} max Largest allowed value.
 * @returns {number} The clamped value.
 */
function clamp(value, min, max) {
  return Math.min(Math.max(value, min), Math.max(min, max));
}

/**
 * Reads previously stored row heights from localStorage.
 *
 * Each side may independently hold a pixel height or `null` (meaning "use the
 * automatic layout"), so a single resized column must not invalidate the
 * other one's saved value.
 * @returns {{left: number | null, right: number | null}} Stored heights; a side
 * is `null` when it was never chosen or the stored value is not a number.
 */
function loadStoredHeights() {
  const fallback = { left: null, right: null };
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY) || "null");
    if (!parsed || typeof parsed !== "object") {
      return fallback;
    }
    // Keep only finite numeric heights; anything else resets to automatic.
    const readSide = (value) => (Number.isFinite(value) ? value : null);
    return { left: readSide(parsed.left), right: readSide(parsed.right) };
  } catch {
    // Corrupted or unavailable storage: fall back to the automatic heights.
    return fallback;
  }
}

/**
 * Stores row heights in localStorage; storage failures are ignored.
 * @param {{left: number | null, right: number | null}} heights Heights in pixels to persist.
 * @returns {void}
 */
function saveStoredHeights(heights) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(heights));
  } catch {
    // Storage may be full or disabled; the layout still works for this session.
  }
}

/**
 * Wires the horizontal splitters of the left and right columns to pointer, keyboard and reset handlers.
 * @param {object} [options] Optional element overrides.
 * @param {HTMLElement} [options.layout] Grid container (defaults to .app-main).
 * @returns {{refresh: () => void} | null} Controller, or null when the splitters are missing.
 */
export function initLayoutRowSplitters({ layout = document.querySelector(".app-main") } = {}) {
  if (!layout) {
    return null;
  }

  const leftColumn = layout.querySelector(".left-column");
  const rightColumn = layout.querySelector(".right-column");
  const leftSplitter = layout.querySelector('[data-row-splitter="left"]');
  const rightSplitter = layout.querySelector('[data-row-splitter="right"]');
  if (!leftColumn || !rightColumn || !leftSplitter || !rightSplitter) {
    return null;
  }

  // Heights in pixels chosen by the user; null means "use the automatic layout".
  let desired = loadStoredHeights();

  /**
   * Reports whether the two-column layout is active, where the row splitters are meaningful.
   * @returns {boolean} True when the wide media query matches.
   */
  function isWide() {
    return window.matchMedia(WIDE_QUERY).matches;
  }

  /**
   * Returns the panel element directly before a splitter (the top/first panel).
   * @param {HTMLElement} splitter Splitter element inside a column.
   * @returns {HTMLElement} Previous sibling element.
   */
  function topPanel(splitter) {
    return splitter.previousElementSibling;
  }

  /**
   * Returns the panel element directly after a splitter (the bottom/second panel).
   * @param {HTMLElement} splitter Splitter element inside a column.
   * @returns {HTMLElement} Next sibling element.
   */
  function bottomPanel(splitter) {
    return splitter.nextElementSibling;
  }

  /**
   * Computes the total height available to the two panels and the splitter in a column.
   * @param {HTMLElement} column Column element.
   * @param {HTMLElement} splitter Splitter element inside the column.
   * @returns {number} Available height in pixels for the two panels.
   */
  function availableHeight(column, splitter) {
    return column.clientHeight - splitter.offsetHeight - 2 * 3.5;
  }

  /**
   * Writes the left column's top-panel height as a CSS variable.
   * @param {number | null} height Height in pixels, or null to use the automatic layout.
   * @returns {void}
   */
  function applyLeft(height) {
    if (height === null) {
      leftColumn.style.removeProperty("--left-top-row");
    } else {
      leftColumn.style.setProperty("--left-top-row", `${height}px`);
    }
  }

  /**
   * Writes the right column's bottom-panel height as a CSS variable.
   * @param {number | null} height Height in pixels, or null to use the automatic layout.
   * @returns {void}
   */
  function applyRight(height) {
    if (height === null) {
      rightColumn.style.removeProperty("--right-bottom-row");
    } else {
      rightColumn.style.setProperty("--right-bottom-row", `${height}px`);
    }
  }

  /**
   * Clamps and applies the desired heights to the current column sizes. Each
   * column is clamped independently from its own stored value, so a change in
   * one column never moves the splitter of the other one.
   * @returns {void}
   */
  function refresh() {
    if (!isWide()) {
      // Single-column layout: clear inline heights so the stacked panels size themselves.
      applyLeft(null);
      applyRight(null);
      return;
    }

    if (desired.left !== null) {
      const bounds = sideBounds("left");
      applyLeft(clamp(desired.left, bounds.min, bounds.max));
    } else {
      applyLeft(null);
    }

    if (desired.right !== null) {
      const bounds = sideBounds("right");
      applyRight(clamp(desired.right, bounds.min, bounds.max));
    } else {
      applyRight(null);
    }
  }

  /**
   * Reads the current rendered height of the panel that the splitter resizes.
   * @param {"left" | "right"} side Which column's splitter is being measured.
   * @returns {number} Current height in pixels.
   */
  function currentHeight(side) {
    if (side === "left") {
      return topPanel(leftSplitter).getBoundingClientRect().height;
    }
    return bottomPanel(rightSplitter).getBoundingClientRect().height;
  }

  /**
   * Computes the allowed range for the resizable panel of a side.
   *
   * The bounds only depend on the constant floors and the current column
   * height: the opposite panel's `scrollHeight` is deliberately NOT used.
   * Panels are scroll containers (`.panel { overflow: hidden }` plus a
   * scrollable `.panel-body`), and measuring their content made the upper
   * bound grow/shrink while dragging. That both swallowed most of the pointer
   * travel (the splitter looked sluggish) and coupled the two columns through
   * the shared clamp. Calling it is cheap and safe once per gesture.
   * @param {"left" | "right"} side Column whose bounds are needed.
   * @returns {{min: number, max: number}} Lower and upper height bounds in pixels.
   */
  function sideBounds(side) {
    if (side === "left") {
      const total = availableHeight(leftColumn, leftSplitter);
      // Left splitter sizes the POSITION panel; JOG below keeps OTHER_MIN.
      return { min: PANEL_MIN, max: Math.max(PANEL_MIN, total - OTHER_MIN) };
    }
    const total = availableHeight(rightColumn, rightSplitter);
    // Right splitter sizes the Commands panel; the files panel above keeps PANEL_MIN.
    return { min: OTHER_MIN, max: Math.max(OTHER_MIN, total - PANEL_MIN) };
  }

  /**
   * Applies one height to a single column only. The opposite column is never
   * re-clamped here: that used to make dragging one splitter move the other
   * one whenever content measurements changed.
   * @param {"left" | "right"} side Column to update.
   * @param {number} height Requested height in pixels (clamped to sideBounds).
   * @param {{persist?: boolean}} [options] When true, stores the value immediately.
   * @returns {number} The height actually applied.
   */
  function applySide(side, height, { persist = true } = {}) {
    const bounds = sideBounds(side);
    const applied = clamp(height, bounds.min, bounds.max);
    desired = { ...desired, [side]: applied };
    if (side === "left") {
      applyLeft(applied);
    } else {
      applyRight(applied);
    }
    if (persist) {
      saveStoredHeights(desired);
    }
    return applied;
  }

  /**
   * Starts a pointer drag on a splitter and tracks movement until release.
   * The drag writes only the own column's CSS variable, coalesced into one
   * update per animation frame, so the gesture feels immediate and never
   * touches the opposite column. Bounds are measured once at pointerdown and
   * the value is persisted once at pointerup.
   * @param {"left" | "right"} side Column whose splitter was pressed.
   * @param {PointerEvent} event Pointer down event.
   * @returns {void}
   */
  function startDrag(side, event) {
    if (!isWide()) {
      return;
    }
    event.preventDefault();
    const splitter = side === "left" ? leftSplitter : rightSplitter;
    const startY = event.clientY;
    const startHeight = currentHeight(side);
    // Measure the allowed range once; re-measuring per move caused the lag
    // (forced layout) and the cross-column coupling (clamps drifting).
    const bounds = sideBounds(side);
    let latest = clamp(startHeight, bounds.min, bounds.max);
    let frame = 0;
    splitter.classList.add("is-dragging");
    splitter.setPointerCapture(event.pointerId);

    /**
     * Queues the newest pointer position; the actual DOM write happens in the
     * animation-frame callback so bursty pointermove events collapse into one
     * layout per frame.
     * @param {PointerEvent} moveEvent Pointer move event.
     * @returns {void}
     */
    const onMove = (moveEvent) => {
      const delta = moveEvent.clientY - startY;
      // Left: dragging down grows the top panel. Right: dragging down shrinks the bottom panel.
      const next = side === "left" ? startHeight + delta : startHeight - delta;
      latest = clamp(next, bounds.min, bounds.max);
      if (!frame) {
        frame = requestAnimationFrame(() => {
          frame = 0;
          if (side === "left") {
            applyLeft(latest);
          } else {
            applyRight(latest);
          }
        });
      }
    };

    /**
     * Ends the drag: flushes the pending frame, commits the desired height,
     * persists it once and removes the listeners.
     * @returns {void}
     */
    const onEnd = () => {
      if (frame) {
        cancelAnimationFrame(frame);
        frame = 0;
      }
      splitter.classList.remove("is-dragging");
      splitter.removeEventListener("pointermove", onMove);
      splitter.removeEventListener("pointerup", onEnd);
      splitter.removeEventListener("pointercancel", onEnd);
      if (splitter.hasPointerCapture(event.pointerId)) {
        splitter.releasePointerCapture(event.pointerId);
      }
      // Single localStorage write per gesture instead of one per pointer move.
      desired = { ...desired, [side]: latest };
      if (side === "left") {
        applyLeft(latest);
      } else {
        applyRight(latest);
      }
      saveStoredHeights(desired);
    };

    splitter.addEventListener("pointermove", onMove);
    splitter.addEventListener("pointerup", onEnd);
    splitter.addEventListener("pointercancel", onEnd);
  }

  /**
   * Moves a splitter with the arrow keys.
   * @param {"left" | "right"} side Column whose splitter has focus.
   * @param {KeyboardEvent} event Key down event.
   * @returns {void}
   */
  function handleKey(side, event) {
    if (!isWide()) {
      return;
    }
    const direction = event.key === "ArrowUp" ? -1 : event.key === "ArrowDown" ? 1 : 0;
    if (direction === 0) {
      return;
    }
    event.preventDefault();
    // Up always makes the top panel smaller and the bottom panel larger.
    const base = currentHeight(side);
    const delta = side === "left" ? direction : -direction;
    applySide(side, base + delta * KEYBOARD_STEP);
  }

  /**
   * Resets a column to its automatic height and removes the saved value.
   * @returns {void}
   */
  function resetLayout() {
    desired = { left: null, right: null };
    refresh();
    saveStoredHeights({ left: null, right: null });
  }

  leftSplitter.addEventListener("pointerdown", (event) => startDrag("left", event));
  rightSplitter.addEventListener("pointerdown", (event) => startDrag("right", event));
  leftSplitter.addEventListener("keydown", (event) => handleKey("left", event));
  rightSplitter.addEventListener("keydown", (event) => handleKey("right", event));
  leftSplitter.addEventListener("dblclick", resetLayout);
  rightSplitter.addEventListener("dblclick", resetLayout);

  window.addEventListener("resize", refresh);
  refresh();

  return { refresh };
}
