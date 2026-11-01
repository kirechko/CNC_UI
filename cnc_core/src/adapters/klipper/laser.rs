//! Laser/spindle safety policy for Klipper's macro and SET_PIN output variants.

use super::{KlipperAdapter, Session};
use crate::{CncResult, SpindleCommand, SpindleDirection};
use serde_json::json;

impl KlipperAdapter {
    /// Enables a documented watchdog only when explicitly configured by the machine owner.
    pub(super) fn laser_watchdog_configured(session: &Session) -> bool {
        let settings = &session.config_settings;
        ["pwm_tool", "output_pin"].into_iter().any(|section| {
            settings
                .get(section)
                .and_then(serde_json::Value::as_object)
                .is_some_and(|objects| {
                    objects.values().any(|value| {
                        value
                            .get("maximum_mcu_duration")
                            .and_then(serde_json::Value::as_f64)
                            .is_some_and(|seconds| seconds > 0.0)
                            || value
                                .get("maximum_mcu_duration")
                                .and_then(serde_json::Value::as_str)
                                .and_then(|value| value.parse::<f64>().ok())
                                .is_some_and(|seconds| seconds > 0.0)
                    })
                })
        })
    }

    /// Applies M3/M4/M5 macro handling or SET_PIN fallback and validates raw PWM bounds.
    pub(super) async fn set_laser_output(
        &self,
        session: &mut Session,
        command: SpindleCommand,
    ) -> CncResult<()> {
        if command.direction == SpindleDirection::Stopped || command.rpm == 0.0 {
            return self.laser_off(session).await;
        }
        if !command.rpm.is_finite() || !(0.0..=1.0).contains(&command.rpm) {
            return Err(crate::ControllerError::new(
                "bad_spindle",
                "Klipper PWM laser power must be between 0 and 1",
            ));
        }
        self.spindle(session, command).await
    }

    /// Sends M5 whenever output safety requires it and surfaces inability to confirm output removal.
    pub(super) async fn enforce_laser_off(&self, session: &mut Session) -> CncResult<()> {
        if session.laser_pin.is_none()
            && !session
                .object_names
                .iter()
                .any(|name| name.eq_ignore_ascii_case("gcode_macro M5"))
        {
            return Ok(());
        }
        self.laser_off(session).await
    }

    /// Performs an emergency stop and then best-effort zero-power script through the RPC path.
    pub(super) async fn emergency_stop_and_disable_laser(
        &self,
        session: &mut Session,
    ) -> CncResult<()> {
        let stop = self.rpc(session, "printer.emergency_stop", json!({})).await;
        let off = self.enforce_laser_off(session).await;
        stop.map(|_| ()).and(off)
    }
}
