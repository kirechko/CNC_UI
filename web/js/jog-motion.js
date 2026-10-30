// JOG motion core: incremental jog requests, continuous (hold-to-move) jog
// start/stop bookkeeping, the stop-all button wiring and the keyboard
// bindings. Extracted verbatim from main.js; all shared jog state lives in
// the injected `ctx` and the connection/status helpers come from `app`.
import { t } from "./i18n.js";
import { initJogKeyboard } from "./jog-keyboard.js";

/**
 * Creates the JOG motion controller and wires its DOM listeners.
 *
 * @param {object} app - Shared application bag exposing `getConnected()` and
 *   `refreshStatus()`.
 * @param {object} ctx - Mutable JOG context from jog-context.js.
 * @returns {object} Motion handle: `jog`, `startContinuousJog`,
 *   `stopAllJog`, `stopContinuousJog`, `bindContinuousButton`.
 */
export function initJogMotion(app, ctx) {
  async function jog(button) {
    if (!app.getConnected() || button.disabled || ctx.motionMode === "continuous") return;

    ctx.jogButtons.forEach((item) => {
      item.disabled = true;
    });
    ctx.feedback.textContent = "";
    ctx.feedback.className = "panel-caption";

    try {
      // Rotary step buttons (A–C) carry their own fixed distance; other buttons use the selects.
      let distance;
      if (button.dataset.distance) {
        distance = Number(button.dataset.distance);
      } else if (ctx.rotaryDistanceSelects?.get(button.dataset.axis)) {
        // Keypad rotary buttons (task): degrees come from the per-axis select.
        distance = Number(ctx.rotaryDistanceSelects.get(button.dataset.axis).value);
      } else {
        const distanceSelect = button.dataset.axis === "Z"
          ? ctx.zDistanceSelect
          : ctx.distanceSelect;
        distance = Number(distanceSelect.value);
      }
      if (button.dataset.xDirection && button.dataset.yDirection) {
        const xDirection = Number(button.dataset.xDirection);
        const yDirection = Number(button.dataset.yDirection);
        await ctx.api.sendDialXyJog(xDirection, yDirection, distance);
        ctx.feedback.textContent = t("jog.feedback.diagonal", {
          x: xDirection > 0 ? "+" : "−",
          y: yDirection > 0 ? "+" : "−",
          distance,
        });
        await app.refreshStatus();
        return;
      }

      const request = {
        axis: button.dataset.axis,
        direction: Number(button.dataset.direction),
        distance,
      };
      const invoke = window.__TAURI__?.core?.invoke;

      if (invoke) {
        await invoke("jog", request);
      } else {
        const response = await fetch("/api/jog", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(request),
        });

        if (!response.ok) {
          const detail = await response.text();
          throw new Error(detail || `HTTP ${response.status}`);
        }
      }

      ctx.feedback.textContent = t("jog.feedback.commandSent", { axis: request.axis });
      await app.refreshStatus();
    } catch (error) {
      ctx.feedback.textContent = t("common.errorPrefix", { message: error.message });
      ctx.feedback.className = "error";
    } finally {
      ctx.jogButtons.forEach((item) => {
        item.disabled = !app.getConnected();
      });
    }
  }

  async function startContinuousJog(axis, direction) {
    if (!app.getConnected()) return;

    if (axis === "X" || axis === "Y") {
      ctx.continuousXy[axis] = direction;
      ctx.activeAxes.add(axis);
      try {
        await ctx.api.sendContinuousXY(
          ctx.continuousXy.X,
          ctx.continuousXy.Y,
        );
      } catch (error) {
        ctx.continuousXy[axis] = 0;
        ctx.activeAxes.delete(axis);
        ctx.feedback.textContent = t("main.jogError.continuous", { message: error.message });
        ctx.feedback.className = "error";
      }
      return;
    }

    if (ctx.activeAxes.has(axis)) return;
    ctx.activeAxes.add(axis);
    try {
      await ctx.api.sendContinuousJog(axis, direction);
    } catch (error) {
      ctx.activeAxes.delete(axis);
      ctx.feedback.textContent = t("main.jogError.continuous", { message: error.message });
      ctx.feedback.className = "error";
    }
  }

  async function stopAllJog() {
    await stopContinuousJog();
    const results = await Promise.allSettled([ctx.api.cancelJog(), ctx.api.cancelMachineCommand()]);
    const failure = results.find((result) => result.status === "rejected");
    if (failure) throw failure.reason;
    ctx.feedback.textContent = t("ui.jogStopped");
    ctx.feedback.className = "panel-caption";
    await app.refreshStatus();
  }

  async function stopContinuousJog(axis = null) {
    const xyAxis = axis === "X" || axis === "Y";
    const stopXy = axis === null
      ? ctx.activeAxes.has("X") || ctx.activeAxes.has("Y")
      : xyAxis && ctx.activeAxes.has(axis);

    if (axis === null || axis === "X") ctx.continuousXy.X = 0;
    if (axis === null || axis === "Y") ctx.continuousXy.Y = 0;

    const axes = axis
      ? [axis].filter((activeAxis) => ctx.activeAxes.has(activeAxis) && activeAxis !== "X" && activeAxis !== "Y")
      : [...ctx.activeAxes].filter((activeAxis) => activeAxis !== "X" && activeAxis !== "Y");

    axes.forEach((activeAxis) => ctx.activeAxes.delete(activeAxis));
    if (axis === null || axis === "X") ctx.activeAxes.delete("X");
    if (axis === null || axis === "Y") ctx.activeAxes.delete("Y");

    if (stopXy) {
      try {
        await ctx.api.sendContinuousXY(
          ctx.continuousXy.X,
          ctx.continuousXy.Y,
        );
      } catch (error) {
        ctx.feedback.textContent = t("main.jogError.stopXy", { message: error.message });
        ctx.feedback.className = "error";
      }
    }

    for (const activeAxis of axes) {
      try {
        await ctx.api.sendContinuousJog(activeAxis, 0);
      } catch (error) {
        ctx.feedback.textContent = t("main.jogError.stop", { message: error.message });
        ctx.feedback.className = "error";
      }
    }
  }

  function bindContinuousButton(button, axis, direction) {
    const diagonalX = Number(button.dataset.xDirection);
    const diagonalY = Number(button.dataset.yDirection);
    const isDiagonal = Number.isInteger(diagonalX) && Number.isInteger(diagonalY);

    const start = () => {
      if (isDiagonal) {
        ctx.continuousXy.X = diagonalX;
        ctx.continuousXy.Y = diagonalY;
        ctx.activeAxes.add("X");
        ctx.activeAxes.add("Y");
        ctx.api.sendContinuousXY(diagonalX, diagonalY).catch((error) => {
          ctx.continuousXy.X = 0;
          ctx.continuousXy.Y = 0;
          ctx.activeAxes.delete("X");
          ctx.activeAxes.delete("Y");
          ctx.feedback.textContent = t("main.jogError.continuous", { message: error.message });
          ctx.feedback.className = "error";
        });
      } else {
        startContinuousJog(axis, direction);
      }
    };

    button.addEventListener("pointerdown", (event) => {
      if (ctx.motionMode !== "continuous" || button.disabled) return;
      event.preventDefault();
      button.setPointerCapture(event.pointerId);
      start();
    });

    const stop = () => {
      if (ctx.motionMode === "continuous") {
        stopContinuousJog(isDiagonal ? null : axis);
      }
    };
    button.addEventListener("pointerup", stop);
    button.addEventListener("pointercancel", stop);
    button.addEventListener("lostpointercapture", stop);

    button.addEventListener("keydown", (event) => {
      if (
        ctx.motionMode !== "continuous" ||
        button.disabled ||
        !["Enter", " "].includes(event.key) ||
        event.repeat
      ) return;

      event.preventDefault();
      startContinuousJog(axis, direction);
    });
    button.addEventListener("keyup", (event) => {
      if (ctx.motionMode === "continuous" && ["Enter", " "].includes(event.key)) {
        event.preventDefault();
        stopContinuousJog(axis);
      }
    });
    button.addEventListener("blur", stop);
  }

  document.querySelectorAll("[data-jog-stop]").forEach((button) => {
    button.addEventListener("click", async () => {
      button.disabled = true;
      try {
        await stopAllJog();
      } catch (error) {
        ctx.feedback.textContent = t("main.jogError.stop", { message: error.message });
        ctx.feedback.className = "error";
      } finally {
        button.disabled = !app.getConnected();
      }
    });
  });

  ctx.jogButtons.forEach((button) => {
    button.addEventListener("click", () => jog(button));
    bindContinuousButton(
      button,
      button.dataset.axis,
      Number(button.dataset.direction),
    );
  });

  initJogKeyboard({
    isConnected: () => app.getConnected(),
    getMotionMode: () => ctx.motionMode,
    getJogButtons: () => ctx.jogButtons,
    isDialogOpen: () => ctx.positionDialog.open || ctx.jogSpeedDialog.open,
    jog,
    startContinuousJog,
    stopContinuousJog,
  });

  // Continuous-mode bindings for the stepped Z column buttons next to the dial.
  document.querySelectorAll("[data-dial-continuous-axis]").forEach((button) => {
    bindContinuousButton(
      button,
      button.dataset.dialContinuousAxis,
      Number(button.dataset.direction),
    );
  });

  return {
    jog,
    startContinuousJog,
    stopAllJog,
    stopContinuousJog,
    bindContinuousButton,
  };
}
