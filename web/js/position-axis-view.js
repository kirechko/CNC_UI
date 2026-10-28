import { t } from "./i18n.js";

const SUPPORTED_AXES = new Set(["X", "Y", "Z", "A", "B", "C"]);

/**
 * Creates one axis row (label, work/machine values and zero/home buttons).
 * @param {string} axis - Axis letter, e.g. "X".
 * @returns {HTMLDivElement} The row element.
 */
function createAxisRow(axis) {
  const lowerAxis = axis.toLowerCase();
  const row = document.createElement("div");
  row.className = "axis-row wpos-only";
  row.dataset.axis = axis;

  const supported = SUPPORTED_AXES.has(axis);
  const setZeroLabel = t("axis.setZero", { axis });
  const homeLabel = t("axis.home", { axis });
  // Tooltip for the clickable work-coordinate tile; recomputed whenever rows
  // are (re)rendered, so locale switches refresh it together with the buttons.
  const workValueTitle = t("html.position.valueTitle", { axis });
  const controls = supported
    ? `<button class="position-action axis-${lowerAxis}" type="button" data-zero-axis="${axis}" title="${setZeroLabel}" aria-label="${setZeroLabel}">
        <svg aria-hidden="true" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7">
          <circle cx="10" cy="10" r="7"></circle>
          <circle cx="10" cy="10" r="1.6" fill="currentColor" stroke="none"></circle>
        </svg>
      </button>
      <button class="position-action axis-${lowerAxis}" type="button" data-home-axis="${axis}" title="${homeLabel}" aria-label="${homeLabel}">
        <svg aria-hidden="true" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M2.5 9.2 10 3l7.5 6.2"></path>
          <path d="M4.5 8v8.5h11V8M8 16.5v-5h4v5"></path>
        </svg>
      </button>`
    : `<span></span><span></span>`;

  row.innerHTML = `
    <span class="axis-label ${lowerAxis}">${axis}</span>
    <strong class="axis-value work position-column" ${supported ? `id="${lowerAxis}" tabindex="0" role="button" title="${workValueTitle}"` : ""}>—</strong>
    <span class="axis-value machine position-column" id="machine-${lowerAxis}">—</span>
    ${controls}
  `;

  return row;
}

/**
 * Renders the per-axis position table and wires its toggles and actions.
 * @param {object} options - Container, toggles and action callbacks.
 * @returns {{render: Function, updateDisplay: Function}}
 */
export function initPositionAxisView({
  container,
  workToggle,
  machineToggle,
  openPosition,
  zeroAxis,
  homeAxis,
  showError,
}) {
  let display = { work: true, machine: true };
  let renderedAxisSignature = null;

  /**
   * Applies the work/machine column visibility to all rows and toggle buttons.
   */
  function updateDisplay() {
    const mode = display.work && display.machine
      ? "both"
      : display.machine
        ? "mpos-only"
        : "wpos-only";

    for (const row of container.querySelectorAll(".axis-row")) {
      row.classList.remove("both", "mpos-only", "wpos-only");
      row.classList.add(mode);
    }

    workToggle.setAttribute("aria-pressed", String(display.work));
    machineToggle.setAttribute("aria-pressed", String(display.machine));
    workToggle.classList.toggle("active", display.work);
    machineToggle.classList.toggle("active", display.machine);
  }

  /**
   * Rebuilds the axis rows when the machine configuration changes.
   * @param {object} configuration - Machine configuration payload.
   */
  function render(configuration) {
    const configuredAxes = Array.isArray(configuration?.axes)
      ? configuration.axes.filter((item) =>
          SUPPORTED_AXES.has(String(item.axis).toUpperCase()),
        )
      : ["X", "Y", "Z", "A", "B"].map((axis) => ({ axis, max_travel: null }));
    const axes = [...new Map(
      configuredAxes.map((item) => [String(item.axis).toUpperCase(), item]),
    ).values()].slice(0, 6);
    const settings = configuration?.settings &&
      typeof configuration.settings === "object"
      ? configuration.settings
      : {};
    const signature = JSON.stringify({
      axes: axes.map((item) => [
        String(item.axis).toUpperCase(),
        item.max_travel,
        item.max_velocity,
        item.acceleration,
        item.steps_per_unit,
      ]),
      settings,
    });


    if (signature === renderedAxisSignature) return;

    renderedAxisSignature = signature;
    container.replaceChildren();

    for (const item of axes) {
      const axis = String(item.axis).toUpperCase();
      const row = createAxisRow(axis);
      container.append(row);

      const value = row.querySelector(".axis-value.work");
      value.addEventListener("click", () => openPosition(axis));
      value.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          openPosition(axis);
        }
      });

      row.querySelector("[data-zero-axis]").addEventListener("click", async () => {
        try {
          await zeroAxis(axis);
        } catch (error) {
          showError(t("axis.zeroError", { message: error.message }));
        }
      });

      row.querySelector("[data-home-axis]").addEventListener("click", async () => {
        try {
          await homeAxis(axis);
        } catch (error) {
          showError(t("axis.homingError", { message: error.message }));
        }
      });
    }

    updateDisplay();
  }

  workToggle.addEventListener("click", () => {
    if (display.work && !display.machine) return;
    display.work = !display.work;
    updateDisplay();
  });

  machineToggle.addEventListener("click", () => {
    if (display.machine && !display.work) return;
    display.machine = !display.machine;
    updateDisplay();
  });

  /**
   * Re-applies localized tooltips/labels to the JS-generated zero/home buttons
   * and clickable work-value tiles. Called on `cnc-locale-changed` because the
   * rows are created with template strings and are not covered by the static
   * `data-i18n-*` attribute pass.
   */
  function refreshTitles() {
    for (const row of container.querySelectorAll(".axis-row")) {
      const axis = row.dataset.axis;
      if (!axis) continue;
      const zero = row.querySelector("[data-zero-axis]");
      const home = row.querySelector("[data-home-axis]");
      const value = row.querySelector(".axis-value.work");
      if (zero) {
        const label = t("axis.setZero", { axis });
        zero.title = label;
        zero.setAttribute("aria-label", label);
      }
      if (home) {
        const label = t("axis.home", { axis });
        home.title = label;
        home.setAttribute("aria-label", label);
      }
      // Only supported axes render the tile as a button (role="button");
      // unsupported ones show a plain value and must stay title-free.
      if (value && value.getAttribute("role") === "button") {
        value.title = t("html.position.valueTitle", { axis });
      }
    }
  }

  updateDisplay();

  return { render, updateDisplay, refreshTitles };
}
