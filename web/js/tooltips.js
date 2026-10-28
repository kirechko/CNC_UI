/*
 * Styled tooltips for the whole UI.
 *
 * Native `title` tooltips are replaced by a single floating bubble styled like
 * the JOG Z buttons. The text is taken from `title` while the pointer is over
 * an element and restored on leave, so `title` keeps working for translations
 * and accessibility tools. Event delegation on the document means elements
 * added later (for example dialogs) are covered without extra wiring.
 */

/** Delay before the bubble appears, in milliseconds. */
const SHOW_DELAY_MS = 450;
/** Gap between the anchor element and the bubble, in pixels. */
const OFFSET_PX = 6;
/** Minimum distance from the viewport edge, in pixels. */
const EDGE_PX = 4;

/**
 * Initializes styled tooltips for every element that has a `title` attribute.
 *
 * @returns {{ destroy: Function }} Handle with destroy() that removes the listeners and bubble.
 */
export function initTooltips() {
  // Single floating element reused for every tooltip.
  const bubble = document.createElement("div");
  bubble.className = "app-tooltip";
  bubble.setAttribute("role", "tooltip");
  bubble.hidden = true;
  document.body.appendChild(bubble);

  let activeElement = null;
  let showTimer = null;

  /**
   * Keeps the stashed tooltip text in sync with runtime title changes.
   *
   * While an element is hovered its title lives in data-tooltip-title (the
   * native attribute is removed to suppress the OS tooltip). But the title can
   * be rewritten underneath us afterwards: the i18n layer applies a new
   * language (applyTranslations or direct `.title =` writes from jog-mode.js,
   * jog-dial-interaction.js, jog-rotary-columns.js, ...), or the dial updates
   * its sector tooltip on every pointer move. Without observation, the bubble
   * would keep showing the stale — often English — text captured at pointer
   * enter, which is exactly the "English hints stay after switching locale"
   * bug. A MutationObserver on the active element's title attribute adopts
   * any fresh text into the stash and repaints a visible bubble immediately.
   *
   * One observer is created lazily and re-attached to the hovered element.
   */
  let titleObserver = null;
  let observedElement = null;

  /**
   * Observer callback: adopt a freshly written title while it is stashed.
   *
   * @param {MutationRecord[]} mutations - Attribute mutations for the observed element.
   * @returns {void}
   */
  function onTitleMutated(mutations) {
    for (const mutation of mutations) {
      if (mutation.type !== "attributes" || mutation.attributeName !== "title") continue;
      const element = mutation.target;
      const fresh = element.getAttribute("title");
      // Only care while the title is stashed; the stash owns the tooltip text.
      if (fresh === null || element.dataset.tooltipTitle === undefined) continue;
      // Adopt the new text (translations, dial hover updates, per-axis labels)
      // and keep a visible bubble in sync. An identical rewrite (for example
      // the status poll refreshing a rotary label) still needs the attribute
      // removal below, otherwise the OS would stack a native tooltip on top
      // of the styled bubble while the element is hovered.
      if (fresh !== element.dataset.tooltipTitle) {
        element.dataset.tooltipTitle = fresh;
        if (element === activeElement && !bubble.hidden) {
          bubble.textContent = fresh;
          positionBubble(element);
        }
      }
      // Drop the native title; the removal re-triggers this callback with
      // fresh === null, which the guard above ignores (no infinite loop).
      element.removeAttribute("title");
    }
  }

  /**
   * Starts observing the title of the element that currently owns the stash.
   *
   * @param {HTMLElement} element - Hovered element.
   * @returns {void}
   */
  function attachObserver(element) {
    if (observedElement === element) return;
    detachObserver();
    if (!titleObserver) {
      titleObserver = new MutationObserver(onTitleMutated);
    }
    observedElement = element;
    titleObserver.observe(element, { attributes: true, attributeFilter: ["title"] });
  }

  /**
   * Stops observing the previously hovered element, if any.
   *
   * @returns {void}
   */
  function detachObserver() {
    if (titleObserver && observedElement) {
      titleObserver.disconnect();
    }
    observedElement = null;
  }

  /**
   * Returns the nearest ancestor (or the element itself) that carries a tooltip text.
   *
   * @param {EventTarget|null} target - Event target.
   * @returns {HTMLElement|null} Element with a title or with a stored tooltip text.
   */
  function findTooltipElement(target) {
    if (!(target instanceof Element)) return null;
    return target.closest("[title], [data-tooltip-title]");
  }

  /**
   * Moves the title text from the element into data-tooltip-title, so the native tooltip is suppressed.
   *
   * @param {HTMLElement} element - Element whose title should be stored.
   * @returns {void}
   */
  function stashTitle(element) {
    if (element.hasAttribute("title")) {
      element.dataset.tooltipTitle = element.getAttribute("title");
      element.removeAttribute("title");
    }
  }

  /**
   * Restores the title attribute from data-tooltip-title and clears the stored value.
   *
   * Localization note: between the stash (pointer enter) and this restore (pointer
   * leave) the title may have been re-written by the i18n layer — `applyTranslations`
   * for [data-i18n-title] elements, or a direct `.title =` assignment from modules
   * like jog-mode.js / jog-rotary-columns.js after a runtime language switch. The
   * MutationObserver above normally adopts such fresh text into the stash already,
   * but as a belt-and-braces guard we skip the write-back when the element carries
   * a different current title, so stale English can never clobber a translation.
   *
   * @param {HTMLElement} element - Element whose title should be restored.
   * @returns {void}
   */
  function restoreTitle(element) {
    if (element.dataset.tooltipTitle === undefined) return;
    const current = element.getAttribute("title");
    if (current === null || current === element.dataset.tooltipTitle) {
      element.setAttribute("title", element.dataset.tooltipTitle);
    }
    delete element.dataset.tooltipTitle;
  }

  /**
   * Positions the bubble above the element, or below it when there is no room above.
   *
   * @param {HTMLElement} element - Anchor element.
   * @returns {void}
   */
  function positionBubble(element) {
    const anchor = element.getBoundingClientRect();
    const size = bubble.getBoundingClientRect();
    let top = anchor.top - size.height - OFFSET_PX;
    if (top < EDGE_PX) top = anchor.bottom + OFFSET_PX;
    let left = anchor.left + anchor.width / 2 - size.width / 2;
    left = Math.min(Math.max(EDGE_PX, left), window.innerWidth - size.width - EDGE_PX);
    bubble.style.top = `${Math.round(top)}px`;
    bubble.style.left = `${Math.round(left)}px`;
  }

  /**
   * Hides the bubble and cancels a pending show.
   *
   * @returns {void}
   */
  function hideBubble() {
    if (showTimer !== null) {
      clearTimeout(showTimer);
      showTimer = null;
    }
    bubble.hidden = true;
    if (activeElement) restoreTitle(activeElement);
    detachObserver();
    activeElement = null;
  }

  /**
   * Shows the bubble for the active element after the show delay.
   *
   * @param {HTMLElement} element - Element that was hovered.
   * @returns {void}
   */
  function scheduleBubble(element) {
    showTimer = setTimeout(() => {
      showTimer = null;
      const text = element.dataset.tooltipTitle;
      if (!text) return;
      bubble.textContent = text;
      bubble.hidden = false;
      positionBubble(element);
    }, SHOW_DELAY_MS);
  }

  /**
   * Starts a tooltip when the pointer enters an element that has a title.
   *
   * @param {PointerEvent} event - pointerover event.
   * @returns {void}
   */
  function onPointerOver(event) {
    const element = findTooltipElement(event.target);
    if (!element || element === activeElement) return;
    hideBubble();
    activeElement = element;
    stashTitle(element);
    // Watch for later title rewrites (locale switch, dial hover updates) so
    // the bubble always reflects the current, localized text.
    attachObserver(element);
    if (element.dataset.tooltipTitle) scheduleBubble(element);
  }

  /**
   * Ends the tooltip when the pointer leaves the active element.
   *
   * @param {PointerEvent} event - pointerout event.
   * @returns {void}
   */
  function onPointerOut(event) {
    if (!activeElement) return;
    const next = event.relatedTarget;
    if (next instanceof Node && activeElement.contains(next)) return;
    hideBubble();
  }

  /**
   * Hides the tooltip on pointer down, key press or scroll so it never lingers.
   *
   * @returns {void}
   */
  function onDismiss() {
    hideBubble();
  }

  document.addEventListener("pointerover", onPointerOver);
  document.addEventListener("pointerout", onPointerOut);
  document.addEventListener("pointerdown", onDismiss);
  document.addEventListener("keydown", onDismiss);
  window.addEventListener("scroll", onDismiss, true);
  window.addEventListener("blur", onDismiss);

  return {
    /**
     * Removes all listeners and the bubble element, restoring any stashed title.
     *
     * @returns {void}
     */
    destroy() {
      hideBubble();
      document.removeEventListener("pointerover", onPointerOver);
      document.removeEventListener("pointerout", onPointerOut);
      document.removeEventListener("pointerdown", onDismiss);
      document.removeEventListener("keydown", onDismiss);
      window.removeEventListener("scroll", onDismiss, true);
      window.removeEventListener("blur", onDismiss);
      if (titleObserver) titleObserver.disconnect();
      bubble.remove();
    },
  };
}
