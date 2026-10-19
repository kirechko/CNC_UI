import { invokeOrFetch } from "./api.js";

export function initPositionControls({ getStatus, refreshStatus }) {
  const dialog = document.querySelector("#position-dialog");
  const valueInput = document.querySelector("#position-value");
  const coordinateSystem = document.querySelector("#coordinate-system");
  const message = document.querySelector("#message");
  let editingAxis = null;

  async function sendPositionAction(command, endpoint, args) {
    return invokeOrFetch(command, args, endpoint, "POST");
  }

  function openDialog(axis) {
    editingAxis = axis;
    const option = coordinateSystem.options[coordinateSystem.selectedIndex];
    const status = getStatus();
    const angular = ["A", "B"].includes(axis);

    document.querySelector("#position-dialog-title").textContent =
      `Ось ${axis} · ${option.text}`;
    document.querySelector("#position-value-label").textContent =
      angular ? "Рабочая координата, °" : "Рабочая координата, mm";
    valueInput.value = status
      ? Number(status[axis.toLowerCase()]).toFixed(3)
      : document.querySelector(`#${axis.toLowerCase()}`).textContent;
    dialog.showModal();
    valueInput.focus();
    valueInput.select();
  }

  async function executePositionAction(action) {
    if (!editingAxis) return;

    const value = Number(valueInput.value);
    if (!Number.isFinite(value) || Math.abs(value) > 100000) {
      valueInput.setCustomValidity("Введите конечное значение в диапазоне ±100000 mm");
      valueInput.reportValidity();
      return;
    }
    valueInput.setCustomValidity("");

    const args = {
      axis: editingAxis,
      system: coordinateSystem.value,
      value,
    };
    const setPosition = action === "set";

    try {
      await sendPositionAction(
        setPosition ? "set_axis_work_value" : "go_to_axis_work_value",
        setPosition ? "/api/position/set" : "/api/position/go",
        args,
      );
      dialog.close();
      message.textContent = setPosition
        ? `Рабочая координата ${editingAxis} задана.`
        : `Команда перемещения ${editingAxis} отправлена.`;
      await refreshStatus();
    } catch (error) {
      message.textContent = `Ошибка позиции: ${error.message}`;
    }
  }

  coordinateSystem.addEventListener("change", async () => {
    try {
      await sendPositionAction(
        "select_coordinate_system",
        "/api/position/select",
        { system: coordinateSystem.value },
      );
      await refreshStatus();
    } catch (error) {
      message.textContent = `Ошибка выбора системы координат: ${error.message}`;
    }
  });

  document.querySelector("#zero-all").addEventListener("click", async () => {
    try {
      await sendPositionAction(
        "zero_all",
        "/api/position/zero",
        { system: coordinateSystem.value },
      );
      await refreshStatus();
    } catch (error) {
      message.textContent = `Ошибка Zero all: ${error.message}`;
    }
  });

  document.querySelector("#home-all").addEventListener("click", async () => {
    try {
      await sendPositionAction("home_all", "/api/position/home-all", {});
      message.textContent = "Запущено homing всех осей.";
    } catch (error) {
      message.textContent = `Ошибка homing: ${error.message}`;
    }
  });

  document.querySelector("#position-cancel").addEventListener("click", () => dialog.close());
  document.querySelector("#position-set").addEventListener("click", () =>
    executePositionAction("set"),
  );
  document.querySelector("#position-go").addEventListener("click", () =>
    executePositionAction("go"),
  );
  valueInput.addEventListener("input", () => valueInput.setCustomValidity(""));

  return {
    openDialog,
    sendPositionAction,
  };
}
