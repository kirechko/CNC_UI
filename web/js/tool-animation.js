export function initToolAnimation({ isActive, advance, render }) {
  let frameId = null;
  let previousFrameTime = null;

  function renderFrame(timestamp) {
    frameId = null;

    if (!isActive()) {
      previousFrameTime = null;
      render();
      return;
    }

    if (previousFrameTime !== null) {
      const elapsed = Math.min((timestamp - previousFrameTime) / 1000, 0.1);
      advance(elapsed);
    }
    previousFrameTime = timestamp;
    render();

    if (isActive()) {
      frameId = window.requestAnimationFrame(renderFrame);
    } else {
      previousFrameTime = null;
    }
  }

  function sync() {
    if (isActive()) {
      if (frameId === null) {
        previousFrameTime = null;
        frameId = window.requestAnimationFrame(renderFrame);
      }
      return;
    }

    if (frameId !== null) {
      window.cancelAnimationFrame(frameId);
      frameId = null;
    }
    previousFrameTime = null;
    render();
  }

  return { sync };
}
