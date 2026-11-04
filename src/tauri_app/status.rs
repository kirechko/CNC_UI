//! Desktop status, command-history, logging, and UI-language commands.

use super::desktop_language;
use crate::web_server::{AppState, LogEntry, StatusResponse};
use serde::Serialize;
use tauri::State;

#[tauri::command]
pub(super) fn get_status(state: State<'_, AppState>) -> Result<StatusResponse, String> {
    let status = {
        let controller = state
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.get_status_snapshot()
    };
    state.record_status(&status);
    let connected_device_id = state.connected_machine_id();
    let devices = state.machine_snapshot();
    let mut response =
        StatusResponse::from(status).with_connection_info(&devices, connected_device_id.as_deref());
    if let Ok(latest) = state.latest_log_messages() {
        response.latest_critical = latest.critical;
        response.latest_other = latest.other;
    }
    Ok(response)
}

#[tauri::command]
pub(super) fn cancel_machine_command(state: State<'_, AppState>) {
    state.cancel_machine_command();
}

#[tauri::command]
pub(super) fn get_command_history(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    state.command_history()
}

#[tauri::command]
pub(super) fn execute_machine_command(
    state: State<'_, AppState>,
    command: String,
) -> Result<String, String> {
    let command = command.trim();
    if command.is_empty()
        || command.len() > 4096
        || command.contains('\n')
        || command.contains('\r')
    {
        return Err(crate::i18n::translate(
            desktop_language(),
            "api.error.commandLength",
            &[],
        ));
    }

    state.execute_command(command)
}

#[tauri::command]
pub(super) fn log_message(
    state: State<'_, AppState>,
    severity: String,
    source: String,
    message: String,
) -> Result<(), String> {
    let severity = severity.trim();
    let source = source.trim();
    let message = message.trim();

    // Severity is an internal English identifier (alarm/error/warning/info).
    let language = desktop_language();
    if !crate::message_log::ALL_SEVERITIES.contains(&severity) {
        return Err(crate::i18n::translate(
            language,
            "api.error.unknownSeverity",
            &[],
        ));
    }
    if source.is_empty() || source.len() > 128 || message.is_empty() || message.len() > 16_384 {
        return Err(crate::i18n::translate(
            language,
            "api.error.invalidSourceMessage",
            &[],
        ));
    }

    state.insert_log(severity, source, message);
    Ok(())
}

#[derive(Serialize)]
pub(super) struct TauriLogPage {
    entries: Vec<LogEntry>,
    total: i64,
}

// Keep the flat argument list so the Tauri command signature matches the frontend invoke payload.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub(super) fn get_log(
    state: State<'_, AppState>,
    severity: Option<String>,
    source: Option<String>,
    search: Option<String>,
    sort: Option<String>,
    descending: Option<bool>,
    limit: Option<usize>,
    offset: Option<usize>,
    time_from_ms: Option<i64>,
    time_to_ms: Option<i64>,
) -> Result<TauriLogPage, String> {
    let query = crate::web_server::LogQuery {
        severity,
        source,
        search,
        sort,
        descending,
        limit,
        offset,
        time_from_ms,
        time_to_ms,
    };
    state.query_log(&query).map(|page| TauriLogPage {
        entries: page.entries,
        total: page.total,
    })
}

/// Returns the raw JSON dictionary embedded for a UI language.
///
/// The desktop webview may run on a custom protocol where fetching
/// `web/locales/*.json` assets is unreliable, so the frontend asks this
/// command for dictionary text instead. Unknown languages produce an error
/// which the frontend treats as "fall back to English".
#[tauri::command]
pub(super) fn get_ui_dictionary(language: String) -> Result<&'static str, String> {
    crate::i18n::raw_dictionary(&language)
        .ok_or_else(|| format!("unsupported UI language: {language}"))
}
