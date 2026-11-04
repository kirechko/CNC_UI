//! Desktop commands for macros, local machine profiles, tools, and preflight.

use super::desktop_language;
use crate::web_server::machine_profiles::{MachineProfile, MachineProfileDocument};
use crate::web_server::tool_library::{ToolLibraryDocument, ToolRecord};
use crate::web_server::tool_types::ToolTypeRecord;
use crate::web_server::{AppState, MacroDefinition};
use tauri::State;

/// Returns the complete user macro list to the desktop frontend.
#[tauri::command]
pub(super) fn get_macros(state: State<'_, AppState>) -> Result<Vec<MacroDefinition>, String> {
    state.list_macros()
}

/// Returns macro folders for desktop tree navigation.
#[tauri::command]
pub(super) fn get_macro_folders(
    state: State<'_, AppState>,
) -> Result<Vec<crate::web_server::MacroFolder>, String> {
    state.list_macro_folders()
}

/// Creates a macro folder in the desktop application.
#[tauri::command]
pub(super) fn create_macro_folder(
    state: State<'_, AppState>,
    folder: crate::web_server::MacroFolder,
) -> Result<(), String> {
    crate::web_server::validate_macro_folder(&folder, desktop_language())?;
    state.create_macro_folder(&folder)
}

/// Deletes a macro folder when it is empty.
#[tauri::command]
pub(super) fn delete_macro_folder(
    state: State<'_, AppState>,
    folder_id: String,
) -> Result<(), String> {
    state.delete_macro_folder(&folder_id)
}

/// Validates and atomically saves the complete user macro list from the desktop frontend.
#[tauri::command]
pub(super) fn put_macros(
    state: State<'_, AppState>,
    macros: Vec<MacroDefinition>,
) -> Result<(), String> {
    crate::web_server::validate_macros(&macros, desktop_language())?;
    state.replace_macros(&macros)
}

/// Lists locally authored machine profiles for the desktop profile editor.
#[tauri::command]
pub(super) fn get_machine_profiles(
    state: State<'_, AppState>,
) -> Result<Vec<MachineProfile>, String> {
    state.list_machine_profiles()
}

/// Returns the profile selected for local preflight checks.
#[tauri::command]
pub(super) fn get_active_machine_profile(
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    state.active_machine_profile_id()
}

/// Selects or clears the profile used by preflight without changing machine configuration.
#[tauri::command]
pub(super) fn select_machine_profile(
    state: State<'_, AppState>,
    profile_id: Option<String>,
) -> Result<(), String> {
    state.select_machine_profile(profile_id.as_deref())
}

/// Returns resolved profile/controller metadata without changing or commanding the connected controller.
#[tauri::command]
pub(super) fn get_resolved_machine_configuration(
    state: State<'_, AppState>,
) -> Result<crate::web_server::machine_profiles::ResolvedConfiguration, String> {
    let selected = state.active_machine_profile_id()?;
    state.resolved_machine_configuration(selected.as_deref())
}

/// Saves one locally authored machine profile without changing the connected controller.
#[tauri::command]
pub(super) fn save_machine_profile(
    state: State<'_, AppState>,
    profile: MachineProfile,
) -> Result<(), String> {
    state.save_machine_profile(&profile)
}

/// Deletes one local machine profile by id.
#[tauri::command]
pub(super) fn delete_machine_profile(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if state.delete_machine_profile(&id)? {
        Ok(())
    } else {
        Err("machine profile does not exist".into())
    }
}

/// Exports all machine profiles in the versioned portable document format.
#[tauri::command]
pub(super) fn export_machine_profiles(
    state: State<'_, AppState>,
) -> Result<MachineProfileDocument, String> {
    state
        .list_machine_profiles()
        .map(crate::web_server::machine_profiles::export_document)
}

/// Imports a validated versioned machine profile document transactionally.
/// Imports a versioned profile JSON document through the standard desktop bridge payload.
#[tauri::command]
pub(super) fn import_machine_profiles(
    state: State<'_, AppState>,
    document: MachineProfileDocument,
) -> Result<(), String> {
    state.import_machine_profiles(&document)
}

/// Lists local tool metadata without sending any tool-change commands.
/// Lists user-authored tool metadata without consulting or commanding the controller.
#[tauri::command]
pub(super) fn get_tools(state: State<'_, AppState>) -> Result<Vec<ToolRecord>, String> {
    state.list_tools()
}

/// Saves one local tool metadata record.
#[tauri::command]
pub(super) fn save_tool(state: State<'_, AppState>, tool: ToolRecord) -> Result<(), String> {
    state.save_tool(&tool)
}

/// Deletes one local tool record by id.
#[tauri::command]
pub(super) fn delete_tool(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if state.delete_tool(&id)? {
        Ok(())
    } else {
        Err("tool does not exist".into())
    }
}

/// Exports all local tools in the versioned portable document format.
#[tauri::command]
pub(super) fn export_tools(state: State<'_, AppState>) -> Result<ToolLibraryDocument, String> {
    state
        .list_tools()
        .map(crate::web_server::tool_library::export_document)
}

/// Imports a validated versioned tool document transactionally.
#[tauri::command]
pub(super) fn import_tools(
    state: State<'_, AppState>,
    document: ToolLibraryDocument,
) -> Result<(), String> {
    state.import_tools(&document)
}

/// Lists tool type definitions from the local SQLite database.
#[tauri::command]
pub(super) fn get_tool_types(state: State<'_, AppState>) -> Result<Vec<ToolTypeRecord>, String> {
    state.list_tool_types()
}

/// Saves one independently stored tool type definition.
#[tauri::command]
pub(super) fn save_tool_type(
    state: State<'_, AppState>,
    tool_type: ToolTypeRecord,
) -> Result<(), String> {
    state.save_tool_type(&tool_type)
}

/// Deletes one tool type definition by its stable local id.
#[tauri::command]
pub(super) fn delete_tool_type(state: State<'_, AppState>, id: String) -> Result<(), String> {
    if state.delete_tool_type(&id)? {
        Ok(())
    } else {
        Err("tool type does not exist".into())
    }
}

/// Runs static preflight using Rosetta IR without contacting or controlling the machine.
#[tauri::command]
pub(super) fn preflight_program(
    state: State<'_, AppState>,
    content: Option<String>,
    profile_id: Option<String>,
) -> Result<crate::web_server::PreflightReport, String> {
    let source = content
        .or_else(|| state.loaded_program_text())
        .ok_or_else(|| "no program content supplied or loaded".to_string())?;
    let active = match profile_id {
        Some(id) => Some(id),
        None => state.active_machine_profile_id()?,
    };
    state.preflight_program(&source, active.as_deref())
}

/// Starts a program only when the operator confirms the exact current static preflight result.
#[tauri::command]
pub(super) fn start_program_confirmed(
    state: State<'_, AppState>,
    fingerprint: String,
    confirmed_review: bool,
) -> Result<(), String> {
    let source = state
        .loaded_program_text()
        .ok_or_else(|| "no program loaded".to_string())?;
    let profile_id = state.active_machine_profile_id()?;
    let report = state.preflight_program(&source, profile_id.as_deref())?;
    if report.blocked {
        return Err("preflight found a known hard blocker; Run remains disabled".into());
    }
    if !confirmed_review || fingerprint != report.fingerprint {
        return Err(
            "preflight report changed or was not confirmed; review the current report before Run"
                .into(),
        );
    }
    state.start_loaded_program(desktop_language(), true)
}
