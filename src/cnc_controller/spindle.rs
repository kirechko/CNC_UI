//! Spindle facade of [`CNCController`]: setpoint bookkeeping and the
//! direction commands queued for the worker.

use super::{CNCController, Command, SpindleDirection};
use crate::cnc_api::{SpindleCommand, SpindleDirection as ApiSpindleDirection};

// Spindle setters are wired through the Tauri commands only, so the axum
// binary may legitimately leave them unused (dead_code allowed as before).
#[allow(dead_code)]
impl CNCController {
    /// Stores the desired RPM; the UI must set a speed before starting.
    pub fn set_spindle_speed(&self, speed: f64) {
        if speed.is_finite() && speed >= 0.0 {
            *self.spindle_speed_setpoint.lock().unwrap() = speed;
        } else {
            eprintln!("[UI] Ignored invalid spindle speed: {speed}");
        }
    }

    /// Starts (or switches) the spindle using the stored setpoint.
    ///
    /// `Off` maps to [`CNCController::stop_spindle`]; a request identical to
    /// the current enabled state is ignored so repeated UI clicks do not
    /// spam the machine.
    pub fn set_spindle_direction(&self, direction: SpindleDirection) {
        if direction == SpindleDirection::Off {
            self.stop_spindle();
            return;
        }

        let snapshot = self.get_status_snapshot();
        let spindle_direction = match snapshot.machine.spindle.direction {
            ApiSpindleDirection::Stopped => SpindleDirection::Off,
            ApiSpindleDirection::Clockwise => SpindleDirection::CW,
            ApiSpindleDirection::CounterClockwise => SpindleDirection::Ccw,
        };
        if snapshot.machine.spindle.enabled && spindle_direction == direction {
            return;
        }

        let rpm = *self.spindle_speed_setpoint.lock().unwrap();
        if rpm <= 0.0 {
            eprintln!("[UI] Set spindle speed above zero before starting.");
            return;
        }

        let direction = match direction {
            SpindleDirection::CW => ApiSpindleDirection::Clockwise,
            SpindleDirection::Ccw => ApiSpindleDirection::CounterClockwise,
            SpindleDirection::Off => ApiSpindleDirection::Stopped,
        };
        self.enqueue(Command::Spindle(SpindleCommand { direction, rpm }));
    }

    /// Enqueues a spindle stop command (S0 / M5 equivalent in the adapter).
    pub fn stop_spindle(&self) {
        self.enqueue(Command::Spindle(SpindleCommand {
            direction: ApiSpindleDirection::Stopped,
            rpm: 0.0,
        }));
    }
}
