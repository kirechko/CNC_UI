import { formatJogStep } from "./jog-step-settings.js";

export function initJogStepDialog({ dialog, inputs, getSteps, onApply }) {
  let editingAxis = "xy";

  function open(axis) {
    editingAxis = axis;
    const values = getSteps(axis);

    dialog.querySelector("h2").textContent =
      `Пользовательские шаги Jog ${axis.toUpperCase()}`;
    dialog.querySelector(".panel-caption").textContent =
      `Укажите четыре возрастающих шага в миллиметрах для перемещений ${axis.toUpperCase()}.`;

    inputs.forEach((input, index) => {
      input.value = formatJogStep(values[index]);
      input.setCustomValidity("");
    });

    dialog.showModal();
    inputs[0].focus();
    inputs[0].select();
  }

  function apply() {
    const values = inputs.map((input) => Number(input.value));
    const invalidIndex = values.findIndex((value, index) =>
      !inputs[index].value.trim() ||
      !Number.isFinite(value) ||
      value <= 0 ||
      value > 100000 ||
      (index > 0 && value <= values[index - 1]),
    );

    inputs.forEach((input) => input.setCustomValidity(""));
    if (invalidIndex !== -1) {
      inputs[invalidIndex].setCustomValidity(
        "Введите положительные шаги по возрастанию, не больше 100000 mm.",
      );
      inputs[invalidIndex].reportValidity();
      return false;
    }

    onApply(editingAxis, values);
    dialog.close();
    return true;
  }

  inputs.forEach((input) => {
    input.addEventListener("input", () => input.setCustomValidity(""));
  });

  return { open, apply };
}
