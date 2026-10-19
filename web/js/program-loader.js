import { invokeOrFetch } from "./api.js";

const MAX_PROGRAM_BYTES = 5 * 1024 * 1024;

export function initProgramLoader({ onLoad, onError }) {
  const loadButton = document.querySelector("#load-program");
  const fileInput = document.querySelector("#program-file");

  loadButton.addEventListener("click", () => fileInput.click());

  fileInput.addEventListener("change", async () => {
    const file = fileInput.files?.[0];
    if (!file) return;

    try {
      if (file.size > MAX_PROGRAM_BYTES) {
        throw new Error("Размер G-code превышает лимит 5 MiB.");
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

  async function loadDefault() {
    try {
      const program = await invokeOrFetch("get_program", null, "/api/program");
      if (program) onLoad(program.name, program.content);
    } catch (error) {
      console.warn("Не удалось получить траекторию:", error);
    }
  }

  return { loadDefault };
}
