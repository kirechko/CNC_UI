//! Desktop program loading, execution, validation, transformation, and macro-run commands.

use super::desktop_language;
use crate::web_server::{load_program as load_program_into_state, AppState, LoadedProgram};
use tauri::State;

#[tauri::command]
pub(super) fn get_program(state: State<'_, AppState>) -> Result<Option<LoadedProgram>, String> {
    let program = state
        .program
        .lock()
        .map_err(|_| "program lock poisoned".to_string())?;
    Ok(program.clone())
}

/// Starts a Rosetta macro run on the desktop (T26): dry or live, positional
/// args follow the HTTP body contract; the run id tails through
/// `get_macro_run_status`.
#[tauri::command]
pub(super) fn run_macro(
    state: State<'_, AppState>,
    id: String,
    dry: bool,
    args: Vec<serde_json::Value>,
) -> Result<String, String> {
    state.run_macro(&id, dry, crate::web_server::json_args_to_values(args))
}

/// Tails a Rosetta macro run (status + log lines from `from`).
#[tauri::command]
pub(super) fn get_macro_run_status(
    state: State<'_, AppState>,
    run_id: String,
    from: usize,
) -> Result<crate::rosetta_host::run::RunStatus, String> {
    state
        .macro_run_status(&run_id, from)
        .ok_or_else(|| format!("unknown macro run \"{run_id}\""))
}

/// Cancels a running Rosetta macro.
#[tauri::command]
pub(super) fn cancel_macro_run(state: State<'_, AppState>, run_id: String) -> Result<(), String> {
    if state.cancel_macro_run(&run_id) {
        Ok(())
    } else {
        Err(format!("unknown macro run \"{run_id}\""))
    }
}

/// Capability validation of the loaded program (or an explicit `content`).
#[tauri::command]
pub(super) fn validate_program(
    state: State<'_, AppState>,
    content: Option<String>,
) -> Result<crate::web_server::ValidationReport, String> {
    let source = match content {
        Some(text) => text,
        None => state
            .loaded_program_text()
            .ok_or_else(|| "no program is loaded".to_string())?,
    };
    state.validate_program(&source)
}

/// Transform preview (plan + was/became artifact) for the loaded program.
#[tauri::command]
pub(super) fn transform_preview(
    state: State<'_, AppState>,
    content: Option<String>,
) -> Result<crate::web_server::PreviewReport, String> {
    let source = match content {
        Some(text) => text,
        None => state
            .loaded_program_text()
            .ok_or_else(|| "no program is loaded".to_string())?,
    };
    state.preview_transform(&source)
}

/// Gated transform apply: runs the plan, reloads the program.
#[tauri::command]
pub(super) fn transform_apply(
    state: State<'_, AppState>,
    plan: rosetta::transform::model::TransformPlan,
    approved: bool,
) -> Result<rosetta::transform::Artifact, String> {
    state.apply_transform(&plan, approved)
}

#[tauri::command]
pub(super) fn load_program(
    state: State<'_, AppState>,
    file_name: String,
    content: String,
) -> Result<LoadedProgram, String> {
    load_program_into_state(&state, &file_name, &content, desktop_language())
        .map_err(|error| error.message)
}

/// Starts the loaded program only after the UI has completed the existing confirmation flow.
#[tauri::command]
pub(super) fn start_program(state: State<'_, AppState>) -> Result<(), String> {
    state.start_loaded_program(desktop_language(), false)
}

#[tauri::command]
pub(super) fn pause_program(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller
        .program_pause()
        .map_err(|error| format!("{}: {}", error.code, error.message))
}

#[tauri::command]
pub(super) fn resume_program(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller
        .program_resume()
        .map_err(|error| format!("{}: {}", error.code, error.message))
}

#[tauri::command]
pub(super) fn abort_program(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller
        .program_abort()
        .map_err(|error| format!("{}: {}", error.code, error.message))
}
