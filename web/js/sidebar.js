import { t } from "./i18n.js";

/**
 * Wires the sidebar tabs (files / macros) and switches the sidebar
 * title and description using translated strings.
 */
export function initSidebarTabs() {
  const filesTab = document.querySelector("#files-tab");
  const macrosTab = document.querySelector("#macros-tab");
  const sidebarTitle = document.querySelector("#sidebar-title");
  const sidebarDescription = document.querySelector("#sidebar-description");
  const sidebarContent = document.querySelector("#sidebar-content");

  if (!filesTab || !macrosTab || !sidebarTitle || !sidebarDescription || !sidebarContent) {
    return;
  }

  /**
   * Activates the given sidebar tab and updates ARIA state and texts.
   * @param {"files" | "macros"} tab - Tab to show.
   */
  function selectSidebarTab(tab) {
    const showFiles = tab === "files";
    filesTab.classList.toggle("active", showFiles);
    macrosTab.classList.toggle("active", !showFiles);
    filesTab.setAttribute("aria-selected", String(showFiles));
    macrosTab.setAttribute("aria-selected", String(!showFiles));
    sidebarContent.setAttribute("aria-labelledby", showFiles ? "files-tab" : "macros-tab");
    sidebarTitle.textContent = showFiles ? t("sidebar.files.title") : t("sidebar.macros.title");
    sidebarDescription.textContent = showFiles
      ? t("sidebar.files.description")
      : t("sidebar.macros.description");
  }

  filesTab.addEventListener("click", () => selectSidebarTab("files"));
  macrosTab.addEventListener("click", () => selectSidebarTab("macros"));
}
