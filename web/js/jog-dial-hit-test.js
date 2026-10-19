const DIAL_OUTER_RADIUS = 0.46;
const DIAL_CENTER_RADIUS = 0.15;
const DIAL_RING_LIMITS = [0.3625, 0.575, 0.7875];

function getDialSector(x, y, width, height) {
  const radius = Math.min(width, height) * DIAL_OUTER_RADIUS;
  const dx = x - width / 2;
  const dy = y - height / 2;
  const normalizedDistance = Math.hypot(dx, dy) / radius;

  if (normalizedDistance < DIAL_CENTER_RADIUS || normalizedDistance > 1) {
    return null;
  }

  let angle = Math.atan2(dy, dx);
  if (angle < 0) angle += Math.PI * 2;

  return Math.floor(angle / (Math.PI / 4) + 0.5) % 8;
}

export function dialSelectionAt(x, y, width, height, stepValues) {
  const sector = getDialSector(x, y, width, height);
  if (sector === null || !Array.isArray(stepValues) || stepValues.length !== 4) {
    return null;
  }

  const radius = Math.min(width, height) * DIAL_OUTER_RADIUS;
  const normalizedDistance = Math.hypot(x - width / 2, y - height / 2) / radius;
  const ring = DIAL_RING_LIMITS.findIndex((limit) => normalizedDistance < limit);
  return { sector, step: stepValues[ring === -1 ? 3 : ring] };
}

export function dialDirectionAt(x, y, width, height) {
  const sector = getDialSector(x, y, width, height);
  return sector === null ? null : { sector };
}

const DIAL_MOVES = [
  [["X", 1]],
  [["X", 1], ["Y", -1]],
  [["Y", -1]],
  [["X", -1], ["Y", -1]],
  [["X", -1]],
  [["X", -1], ["Y", 1]],
  [["Y", 1]],
  [["X", 1], ["Y", 1]],
];

export function dialMoves(selection) {
  return Number.isInteger(selection?.sector)
    ? DIAL_MOVES[selection.sector] || null
    : null;
}
