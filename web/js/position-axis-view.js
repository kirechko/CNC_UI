const SUPPORTED_AXES = new Set(["X", "Y", "Z", "A", "B", "C"]);

function createAxisRow(axis) {
  const lowerAxis = axis.toLowerCase();
  const row = document.createElement("div");
  row.className = "axis-row wpos-only";
  row.dataset.axis = axis;

  const supported = SUPPORTED_AXES.has(axis);
  const controls = supported
    ? `<button class="position-action axis-${lowerAxis}" type="button" data-zero-axis="${axis}" title="Установить ноль ${axis}" aria-label="Установить ноль ${axis}">
        <svg aria-hidden="true" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7">
          <circle cx="10" cy="10" r="7"></circle>
          <circle cx="10" cy="10" r="1.6" fill="currentColor" stroke="none"></circle>
        </svg>
      </button>
      <button class="position-action axis-${lowerAxis}" type="button" data-home-axis="${axis}" title="Home ${axis}" aria-label="Home ${axis}">
        <svg aria-hidden="true" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M2.5 9.2 10 3l7.5 6.2"></path>
          <path d="M4.5 8v8.5h11V8M8 16.5v-5h4v5"></path>
        </svg>
      </button>`
    : `<span></span><span></span>`;

  row.innerHTML = `
    <span class="axis-label ${lowerAxis}">${axis}</span>
    <strong class="axis-value work position-column" ${supported ? `id="${lowerAxis}" tabindex="0" role="button"` : ""}>—</strong>
    <span class="axis-value machine position-column" id="machine-${lowerAxis}">—</span>
    ${controls}
  `;

  return row;
}

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
  const workField = document.querySelector("#machine-work-field");

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

  function render(configuration) {
    const configuredAxes = Array.isArray(configuration?.axes)
      ? configuration.axes.filter((item) =>
          SUPPORTED_AXES.has(String(item.axis).toUpperCase()),
        )
      : ["X", "Y", "Z", "A", "B"].map((axis) => ({ axis, max_travel: null }));
    const axes = [...new Map(
      configuredAxes.map((item) => [String(item.axis).toUpperCase(), item]),
    ).values()].slice(0, 6);
    const signature = JSON.stringify(
      axes.map((item) => [String(item.axis).toUpperCase(), item.max_travel]),
    );

    if (workField) {
      const ranges = axes.map((item) => {
        const axis = String(item.axis).toUpperCase();
        const travel = Number(item.max_travel);
        if (!Number.isFinite(travel) || travel <= 0) return `${axis}: —`;
        const units = ["A", "B", "C"].includes(axis) ? "°" : "mm";
        return `${axis}: ${travel.toFixed(3)} ${units}`;
      });
      workField.textContent = ranges.length
        ? `Максимальный ход · ${ranges.join(" · ")}`
        : "Рабочее поле: конфигурация осей недоступна.";
    }

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
          showError(`Ошибка установки нуля: ${error.message}`);
        }
      });

      row.querySelector("[data-home-axis]").addEventListener("click", async () => {
        try {
          await homeAxis(axis);
        } catch (error) {
          showError(`Ошибка homing: ${error.message}`);
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

  updateDisplay();

  return { render, updateDisplay };
}
