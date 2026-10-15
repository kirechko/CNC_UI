use crate::cnc_api::Axis;
use crate::cnc_controller::CNCController;
use crate::web_server::{create_controller, StatusResponse};
use std::sync::{Arc, Mutex};
use tauri::State;

type SharedController = Arc<Mutex<CNCController>>;

#[tauri::command]
fn get_status(controller: State<'_, SharedController>) -> Result<StatusResponse, String> {
    let controller = controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    Ok(controller.get_status_snapshot().into())
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
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.jog_axis_step(axis, direction, distance);
    Ok(())
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let controller = Arc::new(Mutex::new(create_controller()?));

    tauri::Builder::default()
        .manage(controller)
        .invoke_handler(tauri::generate_handler![get_status, jog])
        .run(tauri::generate_context!())?;

    Ok(())
}
