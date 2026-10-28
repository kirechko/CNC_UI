import { invokeOrFetch } from "./api.js";
import { t } from "./i18n.js";

/** Maximum accepted size of a G-code program in bytes (5 MiB). */
const MAX_PROGRAM_BYTES = 5 * 1024 * 1024;

/**
 * Wires the G-code file picker and loads the default program from the backend.
 * @param {{onLoad: Function, onError: Function}} options Callbacks receiving the program name and content, or an error.
 * @returns {{loadDefault: Function}} Object exposing `loadDefault`, which fetches the currently loaded program.
 */
export function initProgramLoader({ onLoad, onError }) {
  const loadButton = document.querySelector("#load-program");
  const fileInput = document.querySelector("#program-file");

  loadButton.addEventListener("click", () => fileInput.click());

  fileInput.addEventListener("change", async () => {
    const file = fileInput.files?.[0];
    if (!file) return;

    try {
      if (file.size > MAX_PROGRAM_BYTES) {
        throw new Error(t("program.error.tooLarge"));
      }

      const content = await file.text();
      const program = await invokeOrFetch(
        "load_program",
        { fileName: file.name, content },
        "/api/program",
        "POST",
      );
      onLoad(program.name, program.content);
    } catch (error) {
      onError(error);
    } finally {
      fileInput.value = "";
    }
  });

  /**
   * Fetches the program that is already loaded on the backend, if any.
   * Failures are logged to the console and do not interrupt startup.
   * @returns {Promise<void>}
   */
  async function loadDefault() {
    try {
      const program = await invokeOrFetch("get_program", null, "/api/program");
      if (program) onLoad(program.name, program.content);
    } catch (error) {
      console.warn(t("program.error.trajectoryLoad"), error);
    }
  }

  return { loadDefault };
}
