export function initSidebarTabs() {
  const filesTab = document.querySelector("#files-tab");
  const macrosTab = document.querySelector("#macros-tab");
  const sidebarTitle = document.querySelector("#sidebar-title");
  const sidebarDescription = document.querySelector("#sidebar-description");
  const sidebarContent = document.querySelector("#sidebar-content");

  if (!filesTab || !macrosTab || !sidebarTitle || !sidebarDescription || !sidebarContent) {
    return;
  }

  function selectSidebarTab(tab) {
    const showFiles = tab === "files";
    filesTab.classList.toggle("active", showFiles);
    macrosTab.classList.toggle("active", !showFiles);
    filesTab.setAttribute("aria-selected", String(showFiles));
    macrosTab.setAttribute("aria-selected", String(!showFiles));
    sidebarContent.setAttribute("aria-labelledby", showFiles ? "files-tab" : "macros-tab");
    sidebarTitle.textContent = showFiles ? "Файловый менеджер" : "Макросы";
    sidebarDescription.textContent = showFiles
      ? "Просмотр и загрузка файлов пока не подключены. Сейчас доступно пошаговое управление осями."
      : "Запуск и настройка макросов пока не подключены к контроллеру.";
  }

  filesTab.addEventListener("click", () => selectSidebarTab("files"));
  macrosTab.addEventListener("click", () => selectSidebarTab("macros"));
}
