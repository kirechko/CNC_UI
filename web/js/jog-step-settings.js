export function isValidJogStepValues(values) {
  return (
    Array.isArray(values) &&
    values.length === 4 &&
    values.every((value, index) =>
      Number.isFinite(value) &&
      value > 0 &&
      value <= 100000 &&
      (index === 0 || value > values[index - 1]),
    )
  );
}

export function loadJogStepValues(key, fallback) {
  try {
    const values = JSON.parse(localStorage.getItem(key));
    return isValidJogStepValues(values) ? values : fallback;
  } catch {
    return fallback;
  }
}

export function formatJogStep(value) {
  return String(value);
}
