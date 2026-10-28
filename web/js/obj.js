import { t } from "./i18n.js";

/**
 * Parses the text of a Wavefront OBJ file into a list of triangles.
 * Only `v` (vertex) and `f` (face) records are used; other records are ignored.
 * Faces with more than three vertices are triangulated as a fan.
 * Throws a localized Error when a record is malformed or the triangle limit is exceeded.
 * @param {string} text Full OBJ file contents.
 * @returns {number[][][]} Array of triangles, each triangle being three [x, y, z] points.
 */
export function parseObj(text) {
  const vertices = [];
  const triangles = [];

  for (const [lineIndex, originalLine] of text.split(/\r?\n/).entries()) {
    const line = originalLine.trim();
    if (!line || line.startsWith("#")) continue;

    const [keyword, ...values] = line.split(/\s+/);
    if (keyword === "v") {
      if (values.length < 3) {
        throw new Error(t("obj.error.invalidVertex", { line: lineIndex + 1 }));
      }
      const point = values.slice(0, 3).map(Number);
      if (point.some((value) => !Number.isFinite(value))) {
        throw new Error(t("obj.error.invalidCoordinates", { line: lineIndex + 1 }));
      }
      vertices.push(point);
    } else if (keyword === "f") {
      if (values.length < 3) {
        throw new Error(t("obj.error.faceTooFewVertices", { line: lineIndex + 1 }));
      }

      const indices = values.map((token) => {
        const rawIndex = Number.parseInt(token.split("/")[0], 10);
        if (!Number.isInteger(rawIndex) || rawIndex === 0) {
          throw new Error(t("obj.error.invalidFaceIndex", { line: lineIndex + 1 }));
        }
        const index = rawIndex > 0 ? rawIndex - 1 : vertices.length + rawIndex;
        if (index < 0 || index >= vertices.length) {
          throw new Error(t("obj.error.indexOutOfRange", { line: lineIndex + 1 }));
        }
        return index;
      });

      for (let index = 1; index < indices.length - 1; index++) {
        triangles.push([
          vertices[indices[0]],
          vertices[indices[index]],
          vertices[indices[index + 1]],
        ]);
        if (triangles.length > 1_000_000) {
          throw new Error(t("obj.error.tooManyTriangles"));
        }
      }
    }
  }

  return triangles;
}
