use crate::cnc_api::{Axis, SpindleDirection as ApiSpindleDirection};
use crate::cnc_controller::SpindleDirection as UiSpindleDirection;
use crate::web_server::{
    load_program as load_program_into_state, AppState, LoadedProgram, StatusResponse,
};
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
    load_program_into_state(&state, &file_name, &content)
}

#[tauri::command]
fn start_program(state: State<'_, AppState>) -> Result<(), String> {
    state.start_loaded_program()
}

#[tauri::command]
fn pause_program(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.program_pause().map_err(|error| format!("{}: {}", error.code, error.message))
}

#[tauri::command]
fn resume_program(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.program_resume().map_err(|error| format!("{}: {}", error.code, error.message))
}

#[tauri::command]
fn abort_program(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.program_abort().map_err(|error| format!("{}: {}", error.code, error.message))
}

#[tauri::command]
fn set_spindle(
    state: State<'_, AppState>,
    rpm: f64,
    enabled: bool,
    direction: String,
) -> Result<(), String> {
    eprintln!(
        "[Backend/Tauri] set_spindle received: enabled={enabled}, direction={direction}, rpm={rpm}"
    );

    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;

    if !enabled {
        controller.stop_spindle();
        return Ok(());
    }

    if !rpm.is_finite() {
        return Err("Скорость шпинделя должна быть конечным числом".into());
    }

    let status = controller.get_status_snapshot();
    if status.state == "DISCONNECTED" {
        return Err("Нет соединения со станком".into());
    }

    let spindle = status
        .machine_configuration
        .as_ref()
        .map(|configuration| &configuration.spindle)
        .ok_or_else(|| "Конфигурация шпинделя недоступна".to_string())?;
    let minimum = spindle.min_rpm.unwrap_or(0.0);
    let maximum = spindle
        .max_rpm
        .unwrap_or(status.spindle_max_speed);

    if maximum <= 0.0 || rpm < minimum || rpm > maximum {
        return Err(format!(
            "Скорость шпинделя должна быть в диапазоне {minimum:.0}–{maximum:.0} RPM"
        ));
    }

    let direction = match direction.as_str() {
        "CW" => UiSpindleDirection::CW,
        "CCW" => UiSpindleDirection::CCW,
        _ => return Err("Направление шпинделя должно быть CW или CCW".into()),
    };

    controller.set_spindle_speed(rpm);
    controller.set_spindle_direction(direction);
    Ok(())
}

#[tauri::command]
fn jog_continuous(
    state: State<'_, AppState>,
    axis: Axis,
    direction: i8,
) -> Result<(), String> {
    if ![-1, 0, 1].contains(&direction) {
        return Err("direction must be -1, 0, or 1".into());
    }

    let axis = match axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
        Axis::A => 'A',
        Axis::B => 'B',
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

#[tauri::command]
fn select_coordinate_system(state: State<'_, AppState>, system: String) -> Result<(), String> {
    state.select_coordinate_system(&system)
}

#[tauri::command]
fn zero_all(state: State<'_, AppState>, system: String) -> Result<(), String> {
    state.zero_all(&system)
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
    state.set_axis_work_value(axis, &system, value)
}

#[tauri::command]
fn go_to_axis_work_value(
    state: State<'_, AppState>,
    axis: Axis,
    system: String,
    value: f64,
) -> Result<(), String> {
    state.go_to_axis_work_value(axis, &system, value)
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
        || ![0.1, 1.0, 10.0, 100.0].contains(&distance)
    {
        return Err("XY directions must be -1 or 1; distance must be 0.1, 1, 10, or 100 mm".into());
    }

    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.jog_xy_step(x_direction, y_direction, distance);
    Ok(())
}

#[tauri::command]
fn jog(
    state: State<'_, AppState>,
    axis: Axis,
    direction: i8,
    distance: f64,
) -> Result<(), String> {
    if ![-1, 1].contains(&direction) || ![0.1, 1.0, 10.0, 100.0].contains(&distance) {
        return Err("direction must be -1 or 1; distance must be 0.1, 1, 10, or 100 mm".into());
    }

    let axis = match axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
        Axis::A => 'A',
        Axis::B => 'B',
    };

    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.jog_axis_step(axis, direction, distance);
    Ok(())
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    tauri::Builder::default()
        .manage(AppState::new()?)
        .invoke_handler(tauri::generate_handler![
            get_status,
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
            jog,
            jog_xy
        ])
        .run(tauri::generate_context!())?;

    Ok(())
}
