export function parseObj(text) {
  const vertices = [];
  const triangles = [];

  for (const [lineIndex, originalLine] of text.split(/\r?\n/).entries()) {
    const line = originalLine.trim();
    if (!line || line.startsWith("#")) continue;

    const [keyword, ...values] = line.split(/\s+/);
    if (keyword === "v") {
      if (values.length < 3) {
        throw new Error(`OBJ: некорректная вершина в строке ${lineIndex + 1}.`);
      }
      const point = values.slice(0, 3).map(Number);
      if (point.some((value) => !Number.isFinite(value))) {
        throw new Error(`OBJ: некорректные координаты в строке ${lineIndex + 1}.`);
      }
      vertices.push(point);
    } else if (keyword === "f") {
      if (values.length < 3) {
        throw new Error(`OBJ: грань без трёх вершин в строке ${lineIndex + 1}.`);
      }

      const indices = values.map((token) => {
        const rawIndex = Number.parseInt(token.split("/")[0], 10);
        if (!Number.isInteger(rawIndex) || rawIndex === 0) {
          throw new Error(`OBJ: некорректный индекс грани в строке ${lineIndex + 1}.`);
        }
        const index = rawIndex > 0 ? rawIndex - 1 : vertices.length + rawIndex;
        if (index < 0 || index >= vertices.length) {
          throw new Error(`OBJ: индекс вершины вне диапазона в строке ${lineIndex + 1}.`);
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
          throw new Error("OBJ содержит больше 1 000 000 треугольников.");
        }
      }
    }
  }

  return triangles;
}
