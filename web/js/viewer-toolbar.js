const INITIAL_VIEW = Object.freeze({
  yaw: -0.72,
  pitch: 0.62,
  zoom: 1,
  panX: 0,
  panY: 0,
});

export function initViewerToolbar({ camera, render }) {
  const fitButton = document.querySelector("#fit-program");
  const projectionToggle = document.querySelector("#projection-toggle");

  fitButton?.addEventListener("click", () => {
    Object.assign(camera, INITIAL_VIEW);
    render();
  });

  function updateProjectionToggle() {
    if (!projectionToggle) return;

    projectionToggle.dataset.perspective = String(camera.perspective);
    projectionToggle.title = camera.perspective
      ? "Переключить на ортографическую проекцию"
      : "Переключить на перспективную проекцию";
    projectionToggle.setAttribute(
      "aria-label",
      camera.perspective
        ? "Перспективная проекция; переключить на ортографическую"
        : "Ортографическая проекция; переключить на перспективную",
    );
    projectionToggle.classList.toggle("active", camera.perspective);
    projectionToggle.setAttribute("aria-pressed", String(camera.perspective));
  }

  projectionToggle?.addEventListener("click", () => {
    camera.perspective = !camera.perspective;
    updateProjectionToggle();
    render();
  });

  updateProjectionToggle();

  return { updateProjectionToggle };
}
