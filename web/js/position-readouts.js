export function createPositionReadoutUpdater(container) {
  return function updatePositionReadouts(status) {
    for (const row of container.querySelectorAll(".axis-row")) {
      const axis = row.dataset.axis.toLowerCase();
      const workValue = Number(status[axis]);
      const machineValue = Number(status[`machine_${axis}`]);
      row.querySelector(".axis-value.work").textContent =
        Number.isFinite(workValue) ? workValue.toFixed(3) : "—";
      row.querySelector(".axis-value.machine").textContent =
        Number.isFinite(machineValue) ? machineValue.toFixed(3) : "—";
    }
  };
}
