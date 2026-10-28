import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/**
 * Initializes the work-position dialog, coordinate system selector, and zero/home-all buttons.
 * @param {{getStatus: Function, refreshStatus: Function}} options Status accessor and refresh callback.
 * @returns {{openDialog: Function, sendPositionAction: Function}} Dialog opener and generic action sender.
 */
export function initPositionControls({ getStatus, refreshStatus }) {
  const dialog = document.querySelector("#position-dialog");
  const valueInput = document.querySelector("#position-value");
  const coordinateSystem = document.querySelector("#coordinate-system");
  const message = document.querySelector("#message");
  let editingAxis = null;

  /**
   * Sends a POST position action to the backend (Tauri command or HTTP endpoint).
   * @param {string} command Tauri command name.
   * @param {string} endpoint HTTP path used in browser mode.
   * @param {unknown} args Request payload.
   * @returns {Promise<unknown>} Backend response.
   */
  async function sendPositionAction(command, endpoint, args) {
    return invokeOrFetch(command, args, endpoint, "POST");
  }

  /**
   * Opens the work-coordinate dialog for an axis and pre-fills the current value.
   * @param {string} axis Axis letter, for example "X" or "A".
   * @returns {void}
   */
  function openDialog(axis) {
    editingAxis = axis;
    const option = coordinateSystem.options[coordinateSystem.selectedIndex];
    const status = getStatus();
    const angular = ["A", "B", "C"].includes(axis);

    document.querySelector("#position-dialog-title").textContent =
      t("position.dialog.title", { axis, system: option.text });
    document.querySelector("#position-value-label").textContent =
      angular ? t("position.label.angular") : t("position.label.linear");
    valueInput.value = status
      ? Number(status[axis.toLowerCase()]).toFixed(3)
      : document.querySelector(`#${axis.toLowerCase()}`).textContent;
    dialog.showModal();
    valueInput.focus();
    valueInput.select();
  }

  /**
   * Validates the entered value and sends either a "set work value" or a "go to" action.
   * @param {"set"|"go"} action Action to perform on the axis being edited.
   * @returns {Promise<void>}
   */
  async function executePositionAction(action) {
    if (!editingAxis) return;

    const value = Number(valueInput.value);
    if (!Number.isFinite(value) || Math.abs(value) > 100000) {
      valueInput.setCustomValidity(t("position.validation.range"));
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
        ? t("position.set.done", { axis: editingAxis })
        : t("position.go.sent", { axis: editingAxis });
      await refreshStatus();
    } catch (error) {
      message.textContent = t("position.error", { message: error.message });
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
      message.textContent = t("position.coordinateSystem.error", { message: error.message });
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
      message.textContent = t("position.zeroAll.error", { message: error.message });
    }
  });

  document.querySelector("#home-all").addEventListener("click", async () => {
    try {
      await sendPositionAction("home_all", "/api/position/home-all", {});
      message.textContent = t("position.homeAll.started");
    } catch (error) {
      message.textContent = t("position.homeAll.error", { message: error.message });
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
