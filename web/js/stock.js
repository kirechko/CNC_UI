import { t } from "./i18n.js";

/** Maximum number of triangles accepted for a stock model. */
const MAX_TRIANGLES = 1_000_000;
/** Maximum accepted size of a binary or ASCII STL payload in bytes (50 MiB). */
const MAX_STL_BYTES = 50 * 1024 * 1024;

/**
 * Validates triangles and computes the axis-aligned bounding box of the mesh.
 * @param {number[][][]} triangles Triangles, each as three [x, y, z] points.
 * @returns {{triangles: number[][][], min: number[], max: number[]}} Validated mesh with its bounds.
 * @throws {Error} When the triangle count is out of range or a coordinate is not finite.
 */
export function finalizeStockMesh(triangles) {
  if (!triangles.length || triangles.length > MAX_TRIANGLES) {
    throw new Error(t("stock.error.triangleCount"));
  }

  const min = [Infinity, Infinity, Infinity];
  const max = [-Infinity, -Infinity, -Infinity];
  for (const triangle of triangles) {
    for (const point of triangle) {
      if (point.some((value) => !Number.isFinite(value))) {
        throw new Error(t("stock.error.invalidCoordinates"));
      }
      for (let axis = 0; axis < 3; axis++) {
        min[axis] = Math.min(min[axis], point[axis]);
        max[axis] = Math.max(max[axis], point[axis]);
      }
    }
  }

  return { triangles, min, max };
}

/**
 * Converts a legacy Three.js geometry (`vertices` + packed `faces`) into triangles.
 * Quads are split into two triangles.
 * @param {{vertices: number[], faces: number[], uvs?: unknown[][]}} geometry Legacy geometry object.
 * @returns {number[][][]} Triangles, each as three [x, y, z] points.
 * @throws {Error} When the face data is corrupted or references a missing vertex.
 */
function parseThreeLegacyGeometry(geometry) {
  if (!Array.isArray(geometry.vertices) || !Array.isArray(geometry.faces)) {
    throw new Error(t("stock.error.threeNoVerticesFaces"));
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
      throw new Error(t("stock.error.threeFaceListCorrupted"));
    }

    const indices = geometry.faces.slice(offset, offset + vertexCount).map(Number);
    offset += vertexCount;
    if (indices.some((index) => !Number.isInteger(index) || !vertices[index])) {
      throw new Error(t("stock.error.threeFaceIndexMissingVertex"));
    }

    if (isQuad) {
      triangles.push(
        [vertices[indices[0]], vertices[indices[1]], vertices[indices[3]]],
        [vertices[indices[1]], vertices[indices[2]], vertices[indices[3]]],
      );
    } else {
      triangles.push(indices.map((index) => vertices[index]));
    }
    if (triangles.length > MAX_TRIANGLES) {
      throw new Error(t("stock.error.threeTooManyTriangles"));
    }

    // Skip optional per-face data (material, UVs, normals, colors) described by the flags.
    if (flags & 2) offset += 1;
    if (flags & 4) offset += geometry.uvs?.length || 0;
    if (flags & 8) offset += (geometry.uvs?.length || 0) * vertexCount;
    if (flags & 16) offset += 1;
    if (flags & 32) offset += vertexCount;
    if (flags & 64) offset += 1;
    if (flags & 128) offset += vertexCount;

    if (offset > geometry.faces.length) {
      throw new Error(t("stock.error.threeFaceDataCorrupted"));
    }
  }

  return triangles;
}

/**
 * Converts a buffer-based Three.js geometry (`attributes.position` + optional `index`) into triangles.
 * @param {object} geometry Buffer geometry object, optionally wrapped in `data`.
 * @returns {number[][][]} Triangles, each as three [x, y, z] points.
 * @throws {Error} When the position attribute or index data is invalid.
 */
function parseThreeBufferGeometry(geometry) {
  const data = geometry.data || geometry;
  const position = data.attributes?.position;
  const positions = position?.array;
  const itemSize = Number(position?.itemSize) || 3;
  if (!Array.isArray(positions) || itemSize < 3 || positions.length % itemSize !== 0) {
    throw new Error(t("stock.error.threeNoPosition"));
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
    throw new Error(t("stock.error.threeIndexCount"));
  }

  const triangles = [];
  for (let index = 0; index < vertexIndices.length; index += 3) {
    const triangleIndices = vertexIndices.slice(index, index + 3);
    if (triangleIndices.some((vertex) =>
      !Number.isInteger(vertex) || vertex < 0 || vertex >= vertices.length,
    )) {
      throw new Error(t("stock.error.threeIndexMissingVertex"));
    }
    triangles.push(triangleIndices.map((vertex) => vertices[vertex]));
    if (triangles.length > MAX_TRIANGLES) {
      throw new Error(t("stock.error.threeTooManyTriangles"));
    }
  }

  return triangles;
}

/**
 * Parses a Three.js JSON model (legacy or buffer geometry, single or multi-geometry)
 * and returns a validated stock mesh.
 * @param {string} text Raw JSON text of the model.
 * @returns {{triangles: number[][][], min: number[], max: number[]}} Validated mesh with its bounds.
 * @throws {Error} When the JSON is invalid or the geometry is unsupported or corrupted.
 */
export function parseThreeJsJson(text) {
  let model;
  try {
    model = JSON.parse(text);
  } catch {
    throw new Error(t("stock.error.threeInvalidJson"));
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
    if (triangles.length > MAX_TRIANGLES) {
      throw new Error(t("stock.error.threeTooManyTriangles"));
    }
  }

  return finalizeStockMesh(triangles);
}

/**
 * Parses an STL file (binary or ASCII) into a validated stock mesh.
 * Binary layout is detected first by its declared triangle count and byte length.
 * @param {ArrayBuffer} buffer Raw STL file contents.
 * @returns {number[][][]} Triangles, each as three [x, y, z] points, without bounds.
 * @throws {Error} When the file exceeds the size limit or contains invalid data.
 */
export function parseStl(buffer) {
  if (buffer.byteLength > MAX_STL_BYTES) {
    throw new Error(t("stock.error.stlTooLarge"));
  }

  const view = new DataView(buffer);
  const triangleCount = buffer.byteLength >= 84 ? view.getUint32(80, true) : 0;
  const binaryLength = 84 + triangleCount * 50;
  const triangles = [];

  if (
    triangleCount > 0 &&
    triangleCount <= MAX_TRIANGLES &&
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
          throw new Error(t("stock.error.stlInvalidCoordinates"));
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
      throw new Error(t("stock.error.stlParseFailed"));
    }

    const vertexPattern =
      /vertex\s+([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)\s+([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)\s+([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)/gi;
    const vertices = [];
    for (const match of text.matchAll(vertexPattern)) {
      const point = [Number(match[1]), Number(match[2]), Number(match[3])];
      if (point.some((value) => !Number.isFinite(value))) {
        throw new Error(t("stock.error.stlInvalidCoordinates"));
      }
      vertices.push(point);
    }

    if (vertices.length === 0 || vertices.length % 3 !== 0) {
      throw new Error(t("stock.error.stlInvalidMesh"));
    }
    if (vertices.length / 3 > MAX_TRIANGLES) {
      throw new Error(t("stock.error.stlTooManyTriangles"));
    }

    for (let index = 0; index < vertices.length; index += 3) {
      triangles.push(vertices.slice(index, index + 3));
    }
  }

  return finalizeStockMesh(triangles);
}
