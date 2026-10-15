use crate::cnc_api::Axis;
use crate::cnc_controller::CNCController;
use crate::web_server::{load_program, AppState, LoadedProgram, StatusResponse};
use tauri::State;

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> Result<StatusResponse, String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    Ok(controller.get_status_snapshot().into())
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
    load_program(&state, &file_name, &content)
}

#[tauri::command]
fn jog(
    controller: State<'_, SharedController>,
    axis: Axis,
    direction: i8,
    distance: f64,
) -> Result<(), String> {
    if ![-1, 1].contains(&direction) || ![0.1, 1.0, 10.0].contains(&distance) {
        return Err("direction must be -1 or 1; distance must be 0.1, 1, or 10 mm".into());
    }

    let axis = match axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
    };

    let controller = controller
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.jog_axis_step(axis, direction, distance);
    Ok(())
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    tauri::Builder::default()
        .manage(AppState::new()?)
        .invoke_handler(tauri::generate_handler![get_status, get_program, load_program, jog])
        .run(tauri::generate_context!())?;

    Ok(())
}
