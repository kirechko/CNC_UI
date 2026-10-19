import { invokeOrFetch } from "./api.js";

const JOB_ENDPOINTS = {
  start_program: "run",
  pause_program: "pause",
  resume_program: "resume",
  abort_program: "abort",
};

async function sendJobCommand(command) {
  const endpoint = JOB_ENDPOINTS[command];
  if (!endpoint) {
    throw new Error(`Неизвестная команда: ${command}`);
  }

  await invokeOrFetch(
    command,
    null,
    `/api/program/${endpoint}`,
    "POST",
  );
}

export function initJobControls({ getProgram, getMachineState, refreshStatus }) {
  const runButton = document.querySelector("#job-run");
  const pauseButton = document.querySelector("#job-pause");
  const abortButton = document.querySelector("#job-abort");
  const caption = document.querySelector("#job-caption");
  const note = document.querySelector("#job-note");

  runButton.addEventListener("click", async () => {
    runButton.disabled = true;
    note.textContent = "";
    const paused = getMachineState() === "PAUSED";

    try {
      await sendJobCommand(paused ? "resume_program" : "start_program");
      note.textContent = paused
        ? "Отправлена команда продолжения программы."
        : "Отправлена команда запуска программы.";
      await refreshStatus();
    } catch (error) {
      note.textContent = `Ошибка управления программой: ${error.message}`;
    }
  });

  pauseButton.addEventListener("click", async () => {
    pauseButton.disabled = true;
    try {
      await sendJobCommand("pause_program");
      note.textContent = "Отправлена команда паузы.";
      await refreshStatus();
    } catch (error) {
      note.textContent = `Ошибка паузы: ${error.message}`;
    }
  });

  abortButton.addEventListener("click", async () => {
    abortButton.disabled = true;
    try {
      await sendJobCommand("abort_program");
      note.textContent = "Отправлена команда остановки программы.";
      await refreshStatus();
    } catch (error) {
      note.textContent = `Ошибка остановки: ${error.message}`;
    }
  });

  return {
    update(status) {
      const hasProgram = Boolean(getProgram());
      const running = status.state === "READING";
      const paused = status.state === "PAUSED";
      const canControl = Boolean(status.connected) && hasProgram;

      runButton.disabled = !canControl || (running && !paused);
      runButton.title = paused ? "Продолжить программу" : "Запустить программу";
      runButton.setAttribute(
        "aria-label",
        paused ? "Продолжить программу" : "Запустить программу",
      );
      pauseButton.disabled = !canControl || !running;
      abortButton.disabled = !canControl || (!running && !paused);

      if (!hasProgram) {
        caption.textContent = "Нет загруженной программы";
      } else if (running || paused) {
        caption.textContent =
          `${paused ? "Пауза" : "Выполняется"} · ${status.current_line}/${status.total_lines}`;
      } else {
        caption.textContent = `Программа загружена · ${status.total_lines} строк`;
      }
    },
  };
}
