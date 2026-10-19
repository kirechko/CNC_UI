const KEY_BINDINGS = new Map([
  ["ArrowUp", ["Y", 1]],
  ["ArrowDown", ["Y", -1]],
  ["ArrowLeft", ["X", -1]],
  ["ArrowRight", ["X", 1]],
  ["PageUp", ["Z", 1]],
  ["PageDown", ["Z", -1]],
]);

export function initJogKeyboard({
  isConnected,
  getMotionMode,
  getJogButtons,
  isDialogOpen,
  jog,
  startContinuousJog,
  stopContinuousJog,
}) {
  const activeKeys = new Map();

  window.addEventListener("keydown", (event) => {
    const binding = KEY_BINDINGS.get(event.code);
    if (!binding) return;

    const target = event.target;
    if (
      target instanceof HTMLElement &&
      (target.isContentEditable ||
        target.closest("input, select, textarea, button, [contenteditable='true']"))
    ) return;
    if (isDialogOpen()) return;

    event.preventDefault();
    if (activeKeys.has(event.code)) return;
    activeKeys.set(event.code, binding);

    if (!isConnected()) return;
    const [axis, direction] = binding;

    if (getMotionMode() === "continuous") {
      startContinuousJog(axis, direction);
      return;
    }

    const button = getJogButtons().find(
      (item) =>
        item.dataset.axis === axis &&
        Number(item.dataset.direction) === direction,
    );
    if (button) void jog(button);
  });

  window.addEventListener("keyup", (event) => {
    const binding = activeKeys.get(event.code);
    if (!binding) return;

    event.preventDefault();
    activeKeys.delete(event.code);
    if (getMotionMode() === "continuous") {
      stopContinuousJog(binding[0]);
    }
  });

  function releaseActiveKeys() {
    const activeAxes = new Set(
      [...activeKeys.values()].map(([axis]) => axis),
    );
    activeKeys.clear();
    if (activeAxes.size > 0) stopContinuousJog();
  }

  window.addEventListener("blur", releaseActiveKeys);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) releaseActiveKeys();
  });
}
