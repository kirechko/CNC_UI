function finalizeStockMesh(triangles) {
  if (!triangles.length || triangles.length > 1_000_000) {
    throw new Error("Модель должна содержать от 1 до 1 000 000 треугольников.");
  }

  const min = [Infinity, Infinity, Infinity];
  const max = [-Infinity, -Infinity, -Infinity];
  for (const triangle of triangles) {
    for (const point of triangle) {
      if (point.some((value) => !Number.isFinite(value))) {
        throw new Error("Модель содержит некорректные координаты.");
      }
      for (let axis = 0; axis < 3; axis++) {
        min[axis] = Math.min(min[axis], point[axis]);
        max[axis] = Math.max(max[axis], point[axis]);
      }
    }
  }

  return { triangles, min, max };
}

export function parseStl(buffer) {
  if (buffer.byteLength > 50 * 1024 * 1024) {
    throw new Error("Размер STL превышает лимит 50 MiB.");
  }

  const view = new DataView(buffer);
  const triangleCount = buffer.byteLength >= 84 ? view.getUint32(80, true) : 0;
  const binaryLength = 84 + triangleCount * 50;
  const triangles = [];

  if (
    triangleCount > 0 &&
    triangleCount <= 1_000_000 &&
    binaryLength === buffer.byteLength
  ) {
    for (let triangleIndex = 0; triangleIndex < triangleCount; triangleIndex++) {
      const offset = 84 + triangleIndex * 50 + 12;
      const triangle = [];

      for (let vertexIndex = 0; vertexIndex < 3; vertexIndex++) {
        const vertexOffset = offset + vertexIndex * 12;
        const point = [
          view.getFloat32(vertexOffset, true),
          view.getFloat32(vertexOffset + 4, true),
          view.getFloat32(vertexOffset + 8, true),
        ];
        if (point.some((value) => !Number.isFinite(value))) {
          throw new Error("STL содержит некорректные координаты.");
        }
        triangle.push(point);
      }
      triangles.push(triangle);
    }
  } else {
    let text;
    try {
      text = new TextDecoder("utf-8", { fatal: true }).decode(buffer);
    } catch {
      throw new Error("Не удалось распознать STL как ASCII или binary.");
    }

    const vertexPattern =
      /vertex\s+([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)\s+([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)\s+([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)/gi;
    const vertices = [];
    for (const match of text.matchAll(vertexPattern)) {
      const point = [Number(match[1]), Number(match[2]), Number(match[3])];
      if (point.some((value) => !Number.isFinite(value))) {
        throw new Error("STL содержит некорректные координаты.");
      }
      vertices.push(point);
    }

    if (vertices.length === 0 || vertices.length % 3 !== 0) {
      throw new Error("STL не содержит корректную треугольную сетку.");
    }
    if (vertices.length / 3 > 1_000_000) {
      throw new Error("STL содержит слишком много треугольников.");
    }

    for (let index = 0; index < vertices.length; index += 3) {
      triangles.push(vertices.slice(index, index + 3));
    }
  }

  return finalizeStockMesh(triangles);
}
