//! Desktop coordinate-system, homing, zeroing, and jogging commands.

use super::desktop_language;
use crate::cnc_api::Axis;
use crate::web_server::AppState;
use tauri::State;

#[tauri::command]
pub(super) fn jog_continuous(
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
pub(super) fn cancel_jog(state: State<'_, AppState>) -> Result<(), String> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller.cancel_jog();
    Ok(())
}

#[tauri::command]
pub(super) fn jog_continuous_xy(
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
pub(super) fn select_coordinate_system(
    state: State<'_, AppState>,
    system: String,
) -> Result<(), String> {
    state.select_coordinate_system(&system, desktop_language())
}

#[tauri::command]
pub(super) fn zero_all(state: State<'_, AppState>, system: String) -> Result<(), String> {
    state.zero_all(&system, desktop_language())
}

#[tauri::command]
pub(super) fn zero_axis(state: State<'_, AppState>, axis: Axis) -> Result<(), String> {
    state.zero_axis(axis)
}

#[tauri::command]
pub(super) fn home_axis(state: State<'_, AppState>, axis: Axis) -> Result<(), String> {
    state.home_axis(axis)
}

#[tauri::command]
pub(super) fn home_all(state: State<'_, AppState>) -> Result<(), String> {
    state.home_all()
}

#[tauri::command]
pub(super) fn set_axis_work_value(
    state: State<'_, AppState>,
    axis: Axis,
    system: String,
    value: f64,
) -> Result<(), String> {
    state.set_axis_work_value(axis, &system, value, desktop_language())
}

#[tauri::command]
pub(super) fn go_to_axis_work_value(
    state: State<'_, AppState>,
    axis: Axis,
    system: String,
    value: f64,
) -> Result<(), String> {
    state.go_to_axis_work_value(axis, &system, value, desktop_language())
}

#[tauri::command]
pub(super) fn jog_xy(
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
pub(super) fn jog(
    state: State<'_, AppState>,
    axis: Axis,
    direction: i8,
    distance: f64,
) -> Result<(), String> {
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
