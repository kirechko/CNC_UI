use crate::cnc_api::Axis;
use crate::cnc_controller::SpindleDirection as UiSpindleDirection;
use crate::web_server::{
    load_program as load_program_into_state, AppState, LoadedProgram, LogEntry, StatusResponse,
};
use serde::Serialize;
use tauri::State;

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> Result<StatusResponse, String> {
    let status = {
        let controller = state
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.get_status_snapshot()
    };
    state.record_status(&status);
    let mut response = StatusResponse::from(status);
    if let Ok(latest) = state.latest_log_messages() {
        response.latest_critical = latest.critical;
        response.latest_other = latest.other;
    }
    Ok(response)
}

#[tauri::command]
fn cancel_machine_command(state: State<'_, AppState>) {
    state.cancel_machine_command();
}

#[tauri::command]
fn get_command_history(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    state.command_history()
}

#[tauri::command]
fn execute_machine_command(state: State<'_, AppState>, command: String) -> Result<String, String> {
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
fn log_message(
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
struct TauriLogPage {
    entries: Vec<LogEntry>,
    total: i64,
}

// Keep the flat argument list so the Tauri command signature matches the frontend invoke payload.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn get_log(
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

#[tauri::command]
fn get_program(state: State<'_, AppState>) -> Result<Option<LoadedProgram>, String> {
    let program = state
        .program
        .lock()
        .map_err(|_| "program lock poisoned".to_string())?;
    Ok(program.clone())
}

#[tauri::command]
fn load_program(
    state: State<'_, AppState>,
    file_name: String,
    content: String,
) -> Result<LoadedProgram, String> {
    load_program_into_state(&state, &file_name, &content, desktop_language())
        .map_err(|error| error.message)
}

/// Starts the loaded program; errors are localized with the desktop language.
#[tauri::command]
fn start_program(state: State<'_, AppState>) -> Result<(), String> {
    state.start_loaded_program(desktop_language())
}

#[tauri::command]
fn pause_program(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller
        .program_pause()
        .map_err(|error| format!("{}: {}", error.code, error.message))
}

#[tauri::command]
fn resume_program(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller
        .program_resume()
        .map_err(|error| format!("{}: {}", error.code, error.message))
}

#[tauri::command]
fn abort_program(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller
        .program_abort()
        .map_err(|error| format!("{}: {}", error.code, error.message))
}

#[tauri::command]
fn set_spindle(
    state: State<'_, AppState>,
    rpm: f64,
    enabled: bool,
    direction: String,
) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;

    if !enabled {
        controller.stop_spindle();
        return Ok(());
    }

    let language = desktop_language();
    if !rpm.is_finite() {
        return Err(crate::i18n::translate(
            language,
            "api.error.spindleSpeedFinite",
            &[],
        ));
    }

    let status = controller.get_status_snapshot();
    if !status.machine.connected {
        return Err(crate::i18n::translate(language, "api.error.noConnection", &[]));
    }

    let spindle = status
        .machine_configuration
        .as_ref()
        .map(|configuration| &configuration.spindle)
        .ok_or_else(|| {
            crate::i18n::translate(language, "api.error.spindleConfigUnavailable", &[])
        })?;
    let minimum = spindle.min_rpm.unwrap_or(0.0);
    let maximum = spindle.max_rpm.unwrap_or(status.machine.max_spindle_rpm);

    if maximum <= 0.0 || rpm < minimum || rpm > maximum {
        return Err(crate::i18n::translate(
            language,
            "api.error.spindleRange",
            &[
                ("minimum", format!("{minimum:.0}")),
                ("maximum", format!("{maximum:.0}")),
            ],
        ));
    }

    let direction = match direction.as_str() {
        "CW" => UiSpindleDirection::CW,
        "CCW" => UiSpindleDirection::Ccw,
        _ => {
            return Err(crate::i18n::translate(
                language,
                "api.error.spindleDirection",
                &[],
            ))
        }
    };

    controller.set_spindle_speed(rpm);
    controller.set_spindle_direction(direction);
    Ok(())
}

#[tauri::command]
fn jog_continuous(state: State<'_, AppState>, axis: Axis, direction: i8) -> Result<(), String> {
    if ![-1, 0, 1].contains(&direction) {
        return Err("direction must be -1, 0, or 1".into());
    }

    let axis = match axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
        Axis::A => 'A',
        Axis::B => 'B',
        Axis::C => 'C',
    };

    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.jog_axis_continuous(axis, direction);
    Ok(())
}

#[tauri::command]
fn cancel_jog(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.cancel_jog();
    Ok(())
}

#[tauri::command]
fn jog_continuous_xy(
    state: State<'_, AppState>,
    x_direction: i8,
    y_direction: i8,
) -> Result<(), String> {
    if ![-1, 0, 1].contains(&x_direction) || ![-1, 0, 1].contains(&y_direction) {
        return Err("XY directions must be -1, 0, or 1".into());
    }

    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.jog_xy_continuous(x_direction, y_direction);
    Ok(())
}

/// Returns the language used for desktop (Tauri) error messages.
///
/// Tauri commands receive no HTTP headers, so the language is taken from the
/// `CNC_LANG` environment variable and falls back to English when unset or unsupported.
fn desktop_language() -> &'static str {
    crate::i18n::negotiate(std::env::var("CNC_LANG").ok().as_deref(), None)
}

#[tauri::command]
fn select_coordinate_system(state: State<'_, AppState>, system: String) -> Result<(), String> {
    state.select_coordinate_system(&system, desktop_language())
}

#[tauri::command]
fn zero_all(state: State<'_, AppState>, system: String) -> Result<(), String> {
    state.zero_all(&system, desktop_language())
}

#[tauri::command]
fn zero_axis(state: State<'_, AppState>, axis: Axis) -> Result<(), String> {
    state.zero_axis(axis)
}

#[tauri::command]
fn home_axis(state: State<'_, AppState>, axis: Axis) -> Result<(), String> {
    state.home_axis(axis)
}

#[tauri::command]
fn home_all(state: State<'_, AppState>) -> Result<(), String> {
    state.home_all()
}

#[tauri::command]
fn set_axis_work_value(
    state: State<'_, AppState>,
    axis: Axis,
    system: String,
    value: f64,
) -> Result<(), String> {
    state.set_axis_work_value(axis, &system, value, desktop_language())
}

#[tauri::command]
fn go_to_axis_work_value(
    state: State<'_, AppState>,
    axis: Axis,
    system: String,
    value: f64,
) -> Result<(), String> {
    state.go_to_axis_work_value(axis, &system, value, desktop_language())
}

#[tauri::command]
fn jog_xy(
    state: State<'_, AppState>,
    x_direction: i8,
    y_direction: i8,
    distance: f64,
) -> Result<(), String> {
    if ![-1, 1].contains(&x_direction)
        || ![-1, 1].contains(&y_direction)
        || !distance.is_finite()
        || distance <= 0.0
        || distance > 100_000.0
    {
        return Err(crate::i18n::translate(
            desktop_language(),
            "api.error.jogXyRange",
            &[],
        ));
    }

    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.jog_xy_step(x_direction, y_direction, distance);
    Ok(())
}

#[tauri::command]
fn jog(state: State<'_, AppState>, axis: Axis, direction: i8, distance: f64) -> Result<(), String> {
    if ![-1, 1].contains(&direction)
        || !distance.is_finite()
        || distance <= 0.0
        || distance > 100_000.0
    {
        return Err(
            "direction must be -1 or 1; distance must be greater than 0 and at most 100000 mm"
                .into(),
        );
    }

    let axis = match axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
        Axis::A => 'A',
        Axis::B => 'B',
        Axis::C => 'C',
    };

    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.jog_axis_step(axis, direction, distance);
    Ok(())
}

/// Returns the raw JSON dictionary embedded for a UI language.
///
/// The desktop webview may run on a custom protocol where fetching
/// `web/locales/*.json` assets is unreliable, so the frontend asks this
/// command for dictionary text instead. Unknown languages produce an error
/// which the frontend treats as "fall back to English".
#[tauri::command]
fn get_ui_dictionary(language: String) -> Result<&'static str, String> {
    crate::i18n::raw_dictionary(&language)
        .ok_or_else(|| format!("unsupported UI language: {language}"))
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    tauri::Builder::default()
        .manage(AppState::new()?)
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_log,
            get_command_history,
            cancel_machine_command,
            log_message,
            execute_machine_command,
            get_program,
            load_program,
            start_program,
            pause_program,
            resume_program,
            abort_program,
            jog,
            set_spindle,
            jog_continuous,
            jog_continuous_xy,
            cancel_jog,
            select_coordinate_system,
            zero_all,
            zero_axis,
            home_axis,
            home_all,
            set_axis_work_value,
            go_to_axis_work_value,
            jog_xy,
            get_ui_dictionary
        ])
        .run(tauri::generate_context!())?;

    Ok(())
}
