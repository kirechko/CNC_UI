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

function parseThreeLegacyGeometry(geometry) {
  if (!Array.isArray(geometry.vertices) || !Array.isArray(geometry.faces)) {
    throw new Error("Three.js JSON: геометрия не содержит vertices/faces.");
  }

  const vertices = [];
  for (let index = 0; index + 2 < geometry.vertices.length; index += 3) {
    vertices.push(geometry.vertices.slice(index, index + 3).map(Number));
  }

  const triangles = [];
  let offset = 0;
  while (offset < geometry.faces.length) {
    const flags = Number(geometry.faces[offset++]);
    const isQuad = (flags & 1) !== 0;
    const vertexCount = isQuad ? 4 : 3;
    if (offset + vertexCount > geometry.faces.length) {
      throw new Error("Three.js JSON: повреждён список граней.");
    }

    const indices = geometry.faces.slice(offset, offset + vertexCount).map(Number);
    offset += vertexCount;
    if (indices.some((index) => !Number.isInteger(index) || !vertices[index])) {
      throw new Error("Three.js JSON: индекс грани ссылается на отсутствующую вершину.");
    }

    if (isQuad) {
      triangles.push(
        [vertices[indices[0]], vertices[indices[1]], vertices[indices[3]]],
        [vertices[indices[1]], vertices[indices[2]], vertices[indices[3]]],
      );
    } else {
      triangles.push(indices.map((index) => vertices[index]));
    }
    if (triangles.length > 1_000_000) {
      throw new Error("Three.js JSON содержит больше 1 000 000 треугольников.");
    }

    if (flags & 2) offset += 1;
    if (flags & 4) offset += geometry.uvs?.length || 0;
    if (flags & 8) offset += (geometry.uvs?.length || 0) * vertexCount;
    if (flags & 16) offset += 1;
    if (flags & 32) offset += vertexCount;
    if (flags & 64) offset += 1;
    if (flags & 128) offset += vertexCount;

    if (offset > geometry.faces.length) {
      throw new Error("Three.js JSON: повреждены данные граней.");
    }
  }

  return triangles;
}

function parseThreeBufferGeometry(geometry) {
  const data = geometry.data || geometry;
  const position = data.attributes?.position;
  const positions = position?.array;
  const itemSize = Number(position?.itemSize) || 3;
  if (!Array.isArray(positions) || itemSize < 3 || positions.length % itemSize !== 0) {
    throw new Error("Three.js JSON: не найден корректный атрибут position.");
  }

  const vertices = [];
  for (let index = 0; index < positions.length; index += itemSize) {
    vertices.push([
      Number(positions[index]),
      Number(positions[index + 1]),
      Number(positions[index + 2]),
    ]);
  }

  const indices = data.index?.array || data.index;
  const vertexIndices = Array.isArray(indices)
    ? indices.map(Number)
    : vertices.map((_, index) => index);
  if (vertexIndices.length % 3 !== 0) {
    throw new Error("Three.js JSON: количество индексов не кратно трём.");
  }

  const triangles = [];
  for (let index = 0; index < vertexIndices.length; index += 3) {
    const triangleIndices = vertexIndices.slice(index, index + 3);
    if (triangleIndices.some((vertex) =>
      !Number.isInteger(vertex) || vertex < 0 || vertex >= vertices.length,
    )) {
      throw new Error("Three.js JSON: индекс ссылается на отсутствующую вершину.");
    }
    triangles.push(triangleIndices.map((vertex) => vertices[vertex]));
    if (triangles.length > 1_000_000) {
      throw new Error("Three.js JSON содержит больше 1 000 000 треугольников.");
    }
  }

  return triangles;
}

export function parseThreeJsJson(text) {
  let model;
  try {
    model = JSON.parse(text);
  } catch {
    throw new Error("Файл не является корректным JSON Three.js.");
  }

  const geometries = Array.isArray(model.geometries) && model.geometries.length
    ? model.geometries.map((entry) => entry.data || entry)
    : [model.data || model];
  const triangles = [];

  for (const geometry of geometries) {
    if (Array.isArray(geometry.vertices) && Array.isArray(geometry.faces)) {
      triangles.push(...parseThreeLegacyGeometry(geometry));
    } else if (geometry.attributes?.position || geometry.data?.attributes?.position) {
      triangles.push(...parseThreeBufferGeometry(geometry));
    }
    if (triangles.length > 1_000_000) {
      throw new Error("Three.js JSON содержит больше 1 000 000 треугольников.");
    }
  }

  return finalizeStockMesh(triangles);
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
