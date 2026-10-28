// Draggable splitters for the main three-column layout (left | center | right).
// Column widths are written to the CSS custom properties --left-col and --right-col on
// .app-main and persisted in localStorage so the chosen layout survives a reload.

// localStorage key holding the last chosen column widths in pixels.
const STORAGE_KEY = "cnc-ui-layout-columns";

// Minimum widths in pixels that keep buttons and labels from being clipped.
const LEFT_MIN = 220;
const RIGHT_MIN = 200;
const CENTER_MIN = 280;

// Pixels moved per arrow key press when a splitter has keyboard focus.
const KEYBOARD_STEP = 16;

// Media query matching the layout where the right column is visible.
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
 * Reads previously stored column widths from localStorage.
 * @returns {{left: number, right: number} | null} Stored widths or null when absent or invalid.
 */
function loadStoredWidths() {
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY) || "null");
    if (
      parsed &&
      Number.isFinite(parsed.left) &&
      Number.isFinite(parsed.right)
    ) {
      return { left: parsed.left, right: parsed.right };
    }
  } catch {
    // Corrupted or unavailable storage: fall back to the CSS defaults.
  }
  return null;
}

/**
 * Stores column widths in localStorage; storage failures are ignored.
 * @param {{left: number, right: number}} widths Widths in pixels to persist.
 * @returns {void}
 */
function saveStoredWidths(widths) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(widths));
  } catch {
    // Storage may be full or disabled; the layout still works for this session.
  }
}

/**
 * Wires the two column splitters of the main layout to pointer, keyboard and reset handlers.
 * @param {object} [options] Optional element overrides.
 * @param {HTMLElement} [options.layout] Grid container (defaults to .app-main).
 * @returns {{refresh: () => void} | null} Controller, or null when the splitters are missing.
 */
export function initLayoutSplitters({ layout = document.querySelector(".app-main") } = {}) {
  if (!layout) {
    return null;
  }

  const leftSplitter = layout.querySelector('[data-splitter="left"]');
  const rightSplitter = layout.querySelector('[data-splitter="right"]');
  const leftColumn = layout.querySelector(".left-column");
  const rightColumn = layout.querySelector(".right-column");
  const centerColumn = layout.querySelector(".center-column");
  if (!leftSplitter || !rightSplitter || !leftColumn || !rightColumn || !centerColumn) {
    return null;
  }

  // Desired widths requested by the user; re-clamped whenever the viewport changes.
  let desired = loadStoredWidths();

  /**
   * Tells whether the wide layout (with the right column) is active.
   * @returns {boolean} True when the right column and its splitter are shown.
   */
  function isWide() {
    return window.matchMedia(WIDE_QUERY).matches;
  }

  /**
   * Computes the space available to the columns, excluding padding, splitters and gaps.
   * @param {boolean} wide Whether the right column is visible.
   * @returns {number} Pixels that can be split between the columns and the center column.
   */
  function availableSpace(wide) {
    const style = getComputedStyle(layout);
    const padding = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    const gap = parseFloat(style.columnGap) || 0;
    const splitterSize = leftSplitter.offsetWidth || 7;
    // Wide layout: 2 splitters and 4 gaps; narrow layout: 1 splitter and 2 gaps.
    const splitterCount = wide ? 2 : 1;
    const gapCount = wide ? 4 : 2;
    return layout.clientWidth - padding - splitterCount * splitterSize - gapCount * gap;
  }

  /**
   * Clamps a pair of requested widths so every column keeps its minimum and the center keeps CENTER_MIN.
   * @param {number} left Requested left column width in pixels.
   * @param {number} right Requested right column width in pixels.
   * @returns {{left: number, right: number, wide: boolean}} Widths that fit the current viewport.
   */
  function clampWidths(left, right) {
    const wide = isWide();
    const space = availableSpace(wide);
    // The right column is hidden in the narrow layout, so it does not limit the left one.
    const rightReserved = wide ? right : 0;
    const clampedLeft = clamp(left, LEFT_MIN, space - CENTER_MIN - rightReserved);
    const clampedRight = wide
      ? clamp(right, RIGHT_MIN, space - CENTER_MIN - clampedLeft)
      : right;
    return { left: clampedLeft, right: clampedRight, wide };
  }

  /**
   * Writes column widths to the CSS custom properties of the layout.
   * @param {number} left Left column width in pixels.
   * @param {number} right Right column width in pixels.
   * @returns {void}
   */
  function applyWidths(left, right) {
    layout.style.setProperty("--left-col", `${left}px`);
    layout.style.setProperty("--right-col", `${right}px`);
  }

  /**
   * Re-applies the desired (or default) widths to the current viewport.
   * @returns {void}
   */
  function refresh() {
    if (!desired) {
      // Without a stored choice, the CSS defaults already scale with the viewport.
      layout.style.removeProperty("--left-col");
      layout.style.removeProperty("--right-col");
      return;
    }
    const clamped = clampWidths(desired.left, desired.right);
    applyWidths(clamped.left, clamped.right);
  }

  /**
   * Applies a new width pair as the user's desired layout and saves it.
   * @param {number} left Requested left column width in pixels.
   * @param {number} right Requested right column width in pixels.
   * @returns {void}
   */
  function setDesired(left, right) {
    const clamped = clampWidths(left, right);
    desired = { left: clamped.left, right: clamped.right };
    applyWidths(desired.left, desired.right);
    saveStoredWidths(desired);
  }

  /**
   * Starts a pointer drag on a splitter and tracks it until the pointer is released.
   * @param {"left" | "right"} side Which splitter is being dragged.
   * @param {PointerEvent} event The pointerdown event that started the drag.
   * @returns {void}
   */
  function startDrag(side, event) {
    if (event.button !== 0) {
      return;
    }
    event.preventDefault();
    const splitter = event.currentTarget;
    splitter.setPointerCapture(event.pointerId);
    splitter.classList.add("is-dragging");

    const startX = event.clientX;
    const startLeft = leftColumn.getBoundingClientRect().width;
    const startRight = rightColumn.getBoundingClientRect().width;
    let latest = { left: startLeft, right: startRight };

    const onMove = (moveEvent) => {
      const dx = moveEvent.clientX - startX;
      // Left splitter moves the left column edge; right splitter moves the right column edge.
      const left = side === "left" ? startLeft + dx : startLeft;
      const right = side === "right" ? startRight - dx : startRight;
      const clamped = clampWidths(left, right);
      latest = { left: clamped.left, right: clamped.right };
      applyWidths(latest.left, latest.right);
    };

    const onEnd = () => {
      splitter.removeEventListener("pointermove", onMove);
      splitter.removeEventListener("pointerup", onEnd);
      splitter.removeEventListener("pointercancel", onEnd);
      splitter.classList.remove("is-dragging");
      if (splitter.hasPointerCapture(event.pointerId)) {
        splitter.releasePointerCapture(event.pointerId);
      }
      setDesired(latest.left, latest.right);
    };

    splitter.addEventListener("pointermove", onMove);
    splitter.addEventListener("pointerup", onEnd);
    splitter.addEventListener("pointercancel", onEnd);
  }

  /**
   * Moves a splitter with the arrow keys for keyboard users.
   * @param {"left" | "right"} side Which splitter received the key press.
   * @param {KeyboardEvent} event The keydown event.
   * @returns {void}
   */
  function handleKey(side, event) {
    const direction = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (direction === 0) {
      return;
    }
    event.preventDefault();
    const left = leftColumn.getBoundingClientRect().width;
    const right = rightColumn.getBoundingClientRect().width;
    // Right splitter: moving right shrinks the right column; left splitter: moving right grows the left one.
    if (side === "left") {
      setDesired(left + direction * KEYBOARD_STEP, right);
    } else {
      setDesired(left, right - direction * KEYBOARD_STEP);
    }
  }

  /**
   * Restores the CSS default widths when a splitter is double-clicked.
   * @returns {void}
   */
  function resetLayout() {
    desired = null;
    try {
      localStorage.removeItem(STORAGE_KEY);
    } catch {
      // Storage unavailable: only the in-memory layout is reset.
    }
    layout.style.removeProperty("--left-col");
    layout.style.removeProperty("--right-col");
  }

  leftSplitter.addEventListener("pointerdown", (event) => startDrag("left", event));
  rightSplitter.addEventListener("pointerdown", (event) => startDrag("right", event));
  leftSplitter.addEventListener("keydown", (event) => handleKey("left", event));
  rightSplitter.addEventListener("keydown", (event) => handleKey("right", event));
  leftSplitter.addEventListener("dblclick", resetLayout);
  rightSplitter.addEventListener("dblclick", resetLayout);

  // Keep the stored widths within the limits whenever the window size changes.
  window.addEventListener("resize", refresh);

  refresh();

  return { refresh };
}
