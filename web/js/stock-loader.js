import { parseObj } from "./obj.js";
import { finalizeStockMesh, parseStl, parseThreeJsJson } from "./stock.js";
import { t } from "./i18n.js";

/** Maximum accepted size of a stock file in bytes (50 MiB). */
const MAX_STOCK_FILE_BYTES = 50 * 1024 * 1024;

/**
 * Wires the stock file picker: the load button opens the file dialog, and the
 * selected file is parsed by extension (STL, OBJ or Three.js JSON).
 * Parsing errors are passed to `onError`; successful results go to `onLoad`.
 * @param {{input: HTMLInputElement, loadButton: HTMLElement, onLoad: Function, onError: Function}} options
 * @returns {void}
 */
export function initStockLoader({ input, loadButton, onLoad, onError }) {
  loadButton.addEventListener("click", () => input.click());

  input.addEventListener("change", async () => {
    const file = input.files?.[0];
    if (!file) return;

    try {
      if (file.size > MAX_STOCK_FILE_BYTES) {
        throw new Error(t("stock.error.tooLarge"));
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
        throw new Error(t("stock.error.unsupportedFormat"));
      }

      onLoad(mesh, file);
    } catch (error) {
      onError(error);
    } finally {
      input.value = "";
    }
  });
}
