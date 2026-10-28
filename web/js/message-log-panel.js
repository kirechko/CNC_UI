// Message-log panel bridge: owns the two "latest message" lines shown in the
// viewer footer (critical + other) plus the full message-log dialog and its
// date-range calendar. Extracted verbatim from main.js so the status refresher
// only needs a single `refresh(status)` call.
import { t } from "./i18n.js";
import { renderLatestLogLine, initMessageLog } from "./message-log.js";
import { initLogCalendar } from "./log-calendar-control.js";

/**
 * Creates the message-log widgets and the footer latest-message lines.
 *
 * @returns {{ refresh: (status: object) => void }} Panel handle; `refresh`
 *   updates both footer lines and re-queries the open log dialog.
 */
export function initMessageLogPanel() {
  const latestCriticalMessage = document.querySelector("#latest-critical-message");
  const latestOtherMessage = document.querySelector("#latest-other-message");
  // The log table needs the calendar's selected range and the calendar needs
  // to re-query the table on change. The cycle is broken with lazy accessors:
  // both consts are fully initialized before any handler can fire.
  const messageLog = initMessageLog({ getDateRange: () => logCalendar.getRange() });
  const logCalendar = initLogCalendar({
    onChange: () => messageLog.refresh(),
  });

  /**
   * Updates the footer latest-message lines from a fresh status snapshot and
   * quietly re-queries the log dialog if the user currently has it open.
   * @param {object} status - Status snapshot from the backend.
   */
  function refresh(status) {
    renderLatestLogLine(
      latestCriticalMessage,
      status.latest_critical,
      "—",
    );
    renderLatestLogLine(
      latestOtherMessage,
      status.latest_other,
      t("ui.noMessages"),
    );
    messageLog.refreshIfOpen();
  }

  return { refresh };
}
