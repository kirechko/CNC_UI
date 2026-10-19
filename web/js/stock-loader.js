import { parseObj } from "./obj.js";
import { finalizeStockMesh, parseStl, parseThreeJsJson } from "./stock.js";

const MAX_STOCK_FILE_BYTES = 50 * 1024 * 1024;

export function initStockLoader({ input, loadButton, onLoad, onError }) {
  loadButton.addEventListener("click", () => input.click());

  input.addEventListener("change", async () => {
    const file = input.files?.[0];
    if (!file) return;

    try {
      if (file.size > MAX_STOCK_FILE_BYTES) {
        throw new Error("Размер файла заготовки превышает лимит 50 MiB.");
      }

      const extension = file.name.split(".").pop()?.toLowerCase();
      let mesh;

      if (extension === "stl") {
        mesh = parseStl(await file.arrayBuffer());
      } else if (extension === "obj") {
        mesh = finalizeStockMesh(parseObj(await file.text()));
      } else if (extension === "json") {
        mesh = parseThreeJsJson(await file.text());
      } else {
        throw new Error("Поддерживаются форматы STL, OBJ и Three.js JSON.");
      }

      onLoad(mesh, file);
    } catch (error) {
      onError(error);
    } finally {
      input.value = "";
    }
  });
}
