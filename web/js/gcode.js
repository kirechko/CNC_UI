export function parseToolpath(source) {
  const segments = [];
  let position = [0, 0, 0];
  let absolute = true;
  let units = 1;
  let motion = 0;
  let feedRate = 1000;
  let programLine = 0;

  for (const originalLine of source.split(/\r?\n/)) {
    const line = originalLine.replace(/\([^)]*\)/g, "").split(";")[0].trim();
    if (!line) continue;
    programLine += 1;
    const words = [...line.matchAll(/([A-Za-z])\s*([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)/g)]
      .map((match) => [match[1].toUpperCase(), Number(match[2])]);
    const values = new Map(words);
    let nextMotion = motion;

    for (const [letter, value] of words) {
      if (letter !== "G") continue;
      if (value === 90) absolute = true;
      else if (value === 91) absolute = false;
      else if (value === 20) units = 25.4;
      else if (value === 21) units = 1;
      else if ([0, 1, 2, 3].includes(value)) nextMotion = value;
    }

    for (const [letter, value] of words) {
      if (letter === "F" && Number.isFinite(value) && value > 0) {
        feedRate = value * units;
      }
    }

    const target = [...position];
    let hasAxis = false;
    for (const [index, letter] of ["X", "Y", "Z", "A", "B"].entries()) {
      if (!values.has(letter)) continue;
      hasAxis = true;
      const value = values.get(letter) * units;
      target[index] = absolute ? value : position[index] + value;
    }

    if (hasAxis && (nextMotion === 2 || nextMotion === 3) &&
        (values.has("I") || values.has("J"))) {
      const centerX = position[0] + (values.get("I") || 0) * units;
      const centerY = position[1] + (values.get("J") || 0) * units;
      const radius = Math.hypot(position[0] - centerX, position[1] - centerY);

      if (radius > 1e-9) {
        const startAngle = Math.atan2(position[1] - centerY, position[0] - centerX);
        const endAngle = Math.atan2(target[1] - centerY, target[0] - centerX);
        let sweep = endAngle - startAngle;
        if (nextMotion === 2) {
          while (sweep >= 0) sweep -= Math.PI * 2;
        } else {
          while (sweep <= 0) sweep += Math.PI * 2;
        }

        if (Math.hypot(target[0] - position[0], target[1] - position[1]) < 1e-8) {
          sweep = nextMotion === 2 ? -Math.PI * 2 : Math.PI * 2;
        }

        const count = Math.min(384, Math.max(16, Math.ceil(Math.abs(sweep) * radius / 1.5)));
        let previous = position;
        for (let step = 1; step <= count; step++) {
          const t = step / count;
          const angle = startAngle + sweep * t;
          const point = [
            centerX + radius * Math.cos(angle),
            centerY + radius * Math.sin(angle),
            position[2] + (target[2] - position[2]) * t,
          ];
          segments.push({
            from: previous,
            to: point,
            type: nextMotion === 2 ? "arc-cw" : "arc-ccw",
            programLine,
            feedRate,
          });
          previous = point;
        }
      } else {
        segments.push({
          from: position,
          to: target,
          type: "linear",
          programLine,
          feedRate,
        });
      }
    } else if (hasAxis) {
      segments.push({
        from: position,
        to: target,
        type: nextMotion === 0 ? "rapid" : "linear",
        programLine,
        feedRate,
      });
    }

    position = target;
    motion = nextMotion;
  }

  return segments;
}

