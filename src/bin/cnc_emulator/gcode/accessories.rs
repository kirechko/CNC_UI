use super::super::protocol::{api_error, WireError};
use super::super::state::Emulator;
use crate::cnc_api::*;

pub(super) fn handle_m_word(emulator: &mut Emulator, value: f64) -> Result<(), WireError> {
    match value as i32 {
        3 => emulator.set_spindle(SpindleDirection::Clockwise, None),
        4 => emulator.set_spindle(SpindleDirection::CounterClockwise, None),
        5 => emulator.set_spindle(SpindleDirection::Stopped, Some(0.0)),
        7 => {
            emulator.status.mist_coolant_on = true;
            emulator.status.message = Some("Mist coolant ON (simulated)".into());
        }
        8 => {
            emulator.status.flood_coolant_on = true;
            emulator.status.message = Some("Flood coolant ON (simulated)".into());
        }
        9 => {
            emulator.status.mist_coolant_on = false;
            emulator.status.flood_coolant_on = false;
            emulator.status.message = Some("Coolant OFF".into());
        }
        30 => emulator.status.message = Some("Program end M30".into()),
        2 => emulator.status.message = Some("Program end M2".into()),
        other => {
            return Err(api_error(
                "unsupported_gcode",
                format!("unsupported M-code M{other}"),
            ));
        }
    }
    Ok(())
}

pub(super) fn set_spindle(emulator: &mut Emulator, direction: SpindleDirection, rpm: Option<f64>) {
    let requested_rpm = rpm
        .unwrap_or(emulator.status.spindle.commanded_rpm)
        .clamp(0.0, emulator.config.spindle.max_rpm);

    if direction == SpindleDirection::Stopped || requested_rpm <= 0.0 {
        emulator.pending_spindle = None;
        emulator.status.spindle.commanded_rpm = 0.0;

        if emulator.status.spindle.actual_rpm <= 0.01 {
            emulator.status.spindle.actual_rpm = 0.0;
            emulator.status.spindle.direction = SpindleDirection::Stopped;
            emulator.status.spindle.enabled = false;
        } else {
            emulator.status.spindle.enabled = true;
        }
        return;
    }

    let current_direction = emulator.status.spindle.direction;
    let is_running = emulator.status.spindle.actual_rpm > 0.01;

    if is_running && current_direction != direction {
        emulator.pending_spindle = Some((direction, requested_rpm));
        emulator.status.spindle.commanded_rpm = 0.0;
        emulator.status.spindle.enabled = true;
    } else {
        emulator.pending_spindle = None;
        emulator.status.spindle.direction = direction;
        emulator.status.spindle.commanded_rpm = requested_rpm;
        emulator.status.spindle.enabled = true;
    }
}
