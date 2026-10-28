import { t } from "./i18n.js";

const INITIAL_VIEW = Object.freeze({
  yaw: -0.72,
  pitch: 0.62,
  zoom: 1,
  panX: 0,
  panY: 0,
});

/**
 * Wires the viewer toolbar: fit-to-window button and projection toggle.
 * @param {{camera: object, render: Function}} deps
 * @returns {{updateProjectionToggle: Function}}
 */
export function initViewerToolbar({ camera, render }) {
  const fitButton = document.querySelector("#fit-program");
  const projectionToggle = document.querySelector("#projection-toggle");

  fitButton?.addEventListener("click", () => {
    Object.assign(camera, INITIAL_VIEW);
    render();
  });

  /**
   * Syncs the projection toggle's state, tooltip and ARIA label with the camera.
   */
  function updateProjectionToggle() {
    if (!projectionToggle) return;

    projectionToggle.dataset.perspective = String(camera.perspective);
    projectionToggle.title = camera.perspective
      ? t("viewer.projection.toOrthographic")
      : t("viewer.projection.toPerspective");
    projectionToggle.setAttribute(
      "aria-label",
      camera.perspective
        ? t("viewer.projection.perspectiveAria")
        : t("viewer.projection.orthographicAria"),
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
