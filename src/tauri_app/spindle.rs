//! Desktop spindle and laser control commands.

use super::desktop_language;
use crate::cnc_controller::SpindleDirection as UiSpindleDirection;
use crate::web_server::AppState;
use tauri::State;

#[tauri::command]
pub(super) fn set_spindle(
    state: State<'_, AppState>,
    rpm: f64,
    enabled: bool,
    direction: String,
    power: Option<f64>,
    mode: Option<String>,
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
    let status = controller.get_status_snapshot();
    let spindle = status
        .machine_configuration
        .as_ref()
        .map(|configuration| &configuration.spindle)
        .ok_or_else(|| {
            crate::i18n::translate(language, "api.error.spindleConfigUnavailable", &[])
        })?;
    let is_laser = spindle.mode != crate::cnc_api::SpindleMode::Standard;
    let value = if is_laser { power.unwrap_or(rpm) } else { rpm };
    if !value.is_finite() {
        return Err(crate::i18n::translate(
            language,
            "api.error.spindleSpeedFinite",
            &[],
        ));
    }

    if !status.machine.connected {
        return Err(crate::i18n::translate(
            language,
            "api.error.noConnection",
            &[],
        ));
    }

    if let Some(mode) = mode.as_deref() {
        let desired = match mode {
            "constant" => crate::cnc_api::SpindleMode::LaserConstant,
            "dynamic" => crate::cnc_api::SpindleMode::LaserDynamic,
            _ => return Err("mode must be constant or dynamic".into()),
        };
        controller.set_laser_mode(desired);
    }
    let (minimum, maximum) = if is_laser {
        let laser = spindle
            .laser
            .as_ref()
            .ok_or_else(|| "laser config unavailable".to_string())?;
        (laser.min_power, laser.max_power)
    } else {
        (
            spindle.min_rpm.unwrap_or(0.0),
            spindle.max_rpm.unwrap_or(status.machine.max_spindle_rpm),
        )
    };

    if maximum <= 0.0 || value < minimum || value > maximum {
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

    if is_laser && direction == UiSpindleDirection::Ccw {
        return Err("laser output does not support reverse direction".into());
    }
    controller.set_spindle_speed(value);
    controller.set_spindle_direction(direction);
    Ok(())
}

/// Selects a laser power mode through the adapter worker.
#[tauri::command]
pub(super) fn set_laser_mode(state: State<'_, AppState>, mode: String) -> Result<(), String> {
    let mode = match mode.as_str() {
        "constant" => crate::cnc_api::SpindleMode::LaserConstant,
        "dynamic" => crate::cnc_api::SpindleMode::LaserDynamic,
        _ => return Err("mode must be constant or dynamic".into()),
    };
    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    let snapshot = controller.get_status_snapshot();
    let laser = snapshot
        .machine_configuration
        .as_ref()
        .and_then(|configuration| configuration.spindle.laser)
        .ok_or_else(|| "laser is not configured on this machine".to_string())?;
    if mode == crate::cnc_api::SpindleMode::LaserDynamic && !laser.dynamic_power_supported {
        return Err("dynamic laser power is not supported by this machine".into());
    }
    controller.set_laser_mode(mode);
    Ok(())
}
