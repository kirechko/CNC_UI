export function initCameraControls({ canvas, camera, render }) {
  let dragState = null;

  canvas.addEventListener("pointerdown", (event) => {
    const mode = event.button === 0
      ? "orbit"
      : event.button === 1 || event.button === 2
        ? "pan"
        : null;
    if (!mode) return;

    event.preventDefault();
    canvas.setPointerCapture(event.pointerId);
    canvas.classList.add("dragging");
    dragState = {
      mode,
      x: event.clientX,
      y: event.clientY,
      yaw: camera.yaw,
      pitch: camera.pitch,
      panX: camera.panX,
      panY: camera.panY,
    };
  });

  canvas.addEventListener("pointermove", (event) => {
    if (!dragState) return;

    const dx = event.clientX - dragState.x;
    const dy = event.clientY - dragState.y;

    if (dragState.mode === "orbit") {
      camera.yaw = dragState.yaw + dx * 0.01;
      camera.pitch = Math.max(
        -Math.PI / 2 + 0.01,
        Math.min(Math.PI / 2 - 0.01, dragState.pitch + dy * 0.01),
      );
    } else {
      camera.panX = dragState.panX + dx;
      camera.panY = dragState.panY + dy;
    }

    render();
  });

  function endDrag() {
    dragState = null;
    canvas.classList.remove("dragging");
  }

  canvas.addEventListener("pointerup", endDrag);
  canvas.addEventListener("pointercancel", endDrag);
  canvas.addEventListener("lostpointercapture", endDrag);
  canvas.addEventListener("contextmenu", (event) => event.preventDefault());

  canvas.addEventListener("wheel", (event) => {
    event.preventDefault();
    camera.zoom = Math.max(
      0.25,
      Math.min(8, camera.zoom * Math.exp(-event.deltaY * 0.001)),
    );
    render();
  }, { passive: false });
}
