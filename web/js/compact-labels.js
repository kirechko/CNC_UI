// Adaptive single-line ("compact") captions for buttons placed inside resizable panels.
//
// Problem: the "Zero all" / "Home all" buttons of the POSITION panel use long
// captions. When the left column (or the whole window) becomes narrow, such a
// caption either wraps onto two lines — which stretches the button and breaks
// the panel rhythm — or gets clipped. The product decision is to drop the
// qualifier word ("all") instead of re-flowing the text: show "Zero" / "Home".
//
// Markup contract (see web/index.html):
//
//   <div class="position-actions" data-i18n-compact-group>
//     <button ...><span data-i18n="html.position.zeroAll"
//                            data-i18n-compact="html.position.zeroAllShort">…</span></button>
//     ...
//   </div>
//
// `data-i18n` keeps feeding the full caption through `applyTranslations`, while
// `data-i18n-compact` names the locale key of the shortened caption. Because the
// span is styled as a single non-breaking, shrinkable line (see the
// `.position-actions button span` rules in web/css/position.css: `white-space: nowrap;
// min-width: 0; overflow: hidden`), a too-long caption makes its `scrollWidth`
// exceed its `clientWidth`.
//
// Grouping: labels that live inside the same `data-i18n-compact-group` container
// switch together. If ANY caption of the group stops fitting on one line, ALL of
// them take their compact form — so a pair like "Zero all"/"Home all" never
// degrades into a visually inconsistent "Zero all"/"Home".
//
// Stage 3 (icon-only): when even the SHORT caption stops fitting — the panel is
// squeezed very narrow — ALL captions of the group are hidden completely, so the
// buttons keep only their icons. The text stays in the DOM (the span is merely
// `hidden`), the `title` tooltip of the button still explains the action, and
// widening the panel brings the words back automatically.
//
// The controller returns `{ refresh, destroy }`; `refresh` is handy after an
// external rewrite of captions (for example a locale re-application through
// `applyTranslations`), and `destroy` disconnects the ResizeObserver.

import { t } from "./i18n.js";

/**
 * Enable compact-label swapping for every `[data-i18n-compact]` caption in `root`.
 * @param {{ root?: ParentNode }} [options] Subtree to scan; defaults to the whole document.
 * @returns {{ refresh: () => void, destroy: () => void }} Controller handle.
 */
export function initCompactLabels({ root = document } = {}) {
  /** Managed caption elements (the `<span>`s carrying both data attributes). */
  const labels = [...root.querySelectorAll("[data-i18n-compact]")];
  if (!labels.length) {
    // Nothing to manage: return an inert handle so callers do not need guards.
    return { refresh() {}, destroy() {} };
  }

  // Collect labels into groups: the nearest `[data-i18n-compact-group]` ancestor,
  // or the parent element when no explicit group container is marked up.
  /** @type {Map<Element, HTMLElement[]>} group container -> its managed captions */
  const groups = new Map();
  for (const label of labels) {
    const container = label.closest("[data-i18n-compact-group]") ?? label.parentElement;
    if (!groups.has(container)) {
      groups.set(container, []);
    }
    groups.get(container).push(label);
  }

  /**
   * Re-fit one group through three shrinking stages:
   *   1. full captions (also the measurement reference);
   *   2. compact captions when any full one overflows its single line;
   *   3. icon-only (all spans hidden) when any compact one STILL overflows.
   * The decision is shared by the whole group so it never looks half-degraded.
   * @param {HTMLElement[]} list Managed captions sharing one group container.
   * @returns {void}
   */
  function fitGroup(list) {
    // Step 0: a previous pass may have hidden the captions (icon-only stage);
    // they must be visible again before any width measurement is meaningful.
    list.forEach((label) => {
      label.hidden = false;
    });
    // Step 1: put the full texts back; this is also the measurement reference.
    list.forEach((label) => {
      label.textContent = t(label.dataset.i18n);
    });
    // Step 2: reading scrollWidth/clientWidth forces a synchronous layout, so
    // the comparison reflects the current (possibly just-resized) button width.
    const anyOverflows = list.some((label) => label.scrollWidth > label.clientWidth);
    // Step 3: a single shared decision keeps the group visually consistent.
    if (!anyOverflows) {
      return;
    }
    list.forEach((label) => {
      label.textContent = t(label.dataset.i18nCompact);
    });
    // Step 4: even the short caption does not fit — drop the words completely
    // and leave icon-only buttons. `hidden` removes the span from the flex row,
    // so the flex `gap` collapses and the icon re-centers automatically. The
    // button keeps its `title` tooltip, so the action stays discoverable.
    const compactOverflows = list.some((label) => label.scrollWidth > label.clientWidth);
    if (compactOverflows) {
      list.forEach((label) => {
        label.hidden = true;
      });
    }
  }

  /**
   * Re-fit all managed groups (public helper for external callers, e.g. after
   * `applyTranslations` rewrites captions for a new locale).
   * @returns {void}
   */
  function refresh() {
    groups.forEach(fitGroup);
  }

  // Coalesce bursts of ResizeObserver notifications (drag events fire many)
  // into a single measurement pass per animation frame.
  let scheduled = 0;
  const scheduleRefresh = () => {
    if (scheduled) {
      return;
    }
    scheduled = requestAnimationFrame(() => {
      scheduled = 0;
      refresh();
    });
  };

  // Observe the buttons (not the spans): any width change of the panel/column —
  // splitter drags, window resizes, row re-flow — must trigger a group re-fit.
  const observer = new ResizeObserver(scheduleRefresh);
  for (const [container, list] of groups) {
    list.forEach((label) => {
      // Fall back to observing the span itself if it somehow is not inside a button.
      observer.observe(label.closest("button") ?? container);
    });
    fitGroup(list);
  }

  return {
    refresh,
    destroy: () => {
      if (scheduled) {
        cancelAnimationFrame(scheduled);
        scheduled = 0;
      }
      observer.disconnect();
    },
  };
}
