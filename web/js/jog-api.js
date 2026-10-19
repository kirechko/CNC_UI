import { invokeOrFetch } from "./api.js";

export function initJogApi({ isConnected = () => true } = {}) {
  let continuousXyRequest = Promise.resolve();

  function sendContinuousXY(xDirection, yDirection) {
    const request = { xDirection, yDirection };
    continuousXyRequest = continuousXyRequest
      .catch(() => {})
      .then(() =>
        invokeOrFetch(
          "jog_continuous_xy",
          request,
          "/api/jog/continuous-xy",
          "POST",
        ),
      );
    return continuousXyRequest;
  }

  function sendContinuousJog(axis, direction) {
    return invokeOrFetch(
      "jog_continuous",
      { axis, direction },
      "/api/jog/continuous",
      "POST",
    );
  }

  function cancelJog() {
    return invokeOrFetch("cancel_jog", null, "/api/jog/cancel", "POST");
  }

  function cancelMachineCommand() {
    return invokeOrFetch(
      "cancel_machine_command",
      null,
      "/api/command/cancel",
      "POST",
    );
  }

  function sendDialXyJog(xDirection, yDirection, distance) {
    if (!isConnected()) return Promise.resolve();

    return invokeOrFetch(
      "jog_xy",
      { xDirection, yDirection, distance },
      "/api/jog/xy",
      "POST",
    );
  }

  function sendDialJog(axis, direction, distance) {
    if (!isConnected()) return Promise.resolve();

    return invokeOrFetch(
      "jog",
      { axis, direction, distance },
      "/api/jog",
      "POST",
    );
  }

  return {
    sendContinuousXY,
    sendContinuousJog,
    cancelJog,
    cancelMachineCommand,
    sendDialXyJog,
    sendDialJog,
  };
}
