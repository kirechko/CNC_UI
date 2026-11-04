//! Tauri desktop bridge; command definitions are grouped by domain while this parent
//! preserves the established handler registration and application entry point.

mod catalog;
mod positioning;
mod program;
mod spindle;
mod status;

use self::catalog::*;
use self::positioning::*;
use self::program::*;
use self::spindle::*;
use self::status::*;
use crate::web_server::AppState;

/// Returns the language used for desktop (Tauri) error messages.
///
/// Tauri commands receive no HTTP headers, so the language is taken from the
/// `CNC_LANG` environment variable and falls back to English when unset or unsupported.
fn desktop_language() -> &'static str {
    crate::i18n::negotiate(std::env::var("CNC_LANG").ok().as_deref(), None)
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
            get_macros,
            get_macro_folders,
            create_macro_folder,
            delete_macro_folder,
            put_macros,
            get_machine_profiles,
            get_active_machine_profile,
            select_machine_profile,
            get_resolved_machine_configuration,
            save_machine_profile,
            delete_machine_profile,
            export_machine_profiles,
            import_machine_profiles,
            get_tools,
            save_tool,
            delete_tool,
            export_tools,
            import_tools,
            get_tool_types,
            save_tool_type,
            delete_tool_type,
            preflight_program,
            start_program_confirmed,
            run_macro,
            get_macro_run_status,
            cancel_macro_run,
            validate_program,
            transform_preview,
            transform_apply,
            load_program,
            start_program,
            set_laser_mode,
            pause_program,
            resume_program,
            abort_program,
            jog,
            set_spindle,
            set_laser_mode,
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
