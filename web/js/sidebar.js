/**
 * Wires the files/macros tabs and keeps the corresponding panel and ARIA state in sync.
 */
export function initSidebarTabs() {
  const filesTab = document.querySelector("#files-tab");
  const macrosTab = document.querySelector("#macros-tab");
  const filesPanel = document.querySelector("#files-panel");
  const macrosPanel = document.querySelector("#macros-panel");
  const sidebarContent = document.querySelector("#sidebar-content");

  if (!filesTab || !macrosTab || !filesPanel || !macrosPanel || !sidebarContent) return;

  /**
   * Selects one sidebar view, updates accessibility state, and notifies the active panel.
   * @param {"files" | "macros"} tab - The sidebar view to display.
   */
  function selectSidebarTab(tab) {
    const showFiles = tab === "files";
    filesTab.classList.toggle("active", showFiles);
    macrosTab.classList.toggle("active", !showFiles);
    filesTab.setAttribute("aria-selected", String(showFiles));
    macrosTab.setAttribute("aria-selected", String(!showFiles));
    filesPanel.hidden = !showFiles;
    macrosPanel.hidden = showFiles;
    sidebarContent.setAttribute("aria-labelledby", showFiles ? "files-tab" : "macros-tab");
    window.dispatchEvent(new CustomEvent("cnc-sidebar-tab-changed", { detail: { tab } }));
  }

  filesTab.addEventListener("click", () => selectSidebarTab("files"));
  macrosTab.addEventListener("click", () => selectSidebarTab("macros"));
  selectSidebarTab(filesTab.getAttribute("aria-selected") === "true" ? "files" : "macros");
}
