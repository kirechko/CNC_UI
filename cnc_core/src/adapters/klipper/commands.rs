//! Klipper G-code commands and print lifecycle operations.

use super::{KlipperAdapter, Session};
use crate::{Axis, CncResult, ControllerError, JogRequest, SpindleCommand, SpindleDirection};
use serde_json::{json, Value};

impl KlipperAdapter {
    /// Sends M5 or sets the configured output pin to zero and updates the cached spindle state.
    pub(super) async fn laser_off(&self, session: &mut Session) -> CncResult<()> {
        if session
            .object_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("gcode_macro M5"))
        {
            self.script(session, "M5").await?;
        } else if let Some(pin) = &session.laser_pin {
            self.script(session, &format!("SET_PIN PIN={pin} VALUE=0"))
                .await?;
        }
        session.spindle_command = SpindleCommand {
            direction: SpindleDirection::Stopped,
            rpm: 0.0,
        };
        Ok(())
    }

    /// Runs a G-code script and returns Moonraker's immediate result.
    pub(super) async fn script(&self, session: &mut Session, script: &str) -> CncResult<()> {
        self.rpc_connected(session, "printer.gcode.script", json!({"script": script}))
            .await
            .map(|_| ())
    }

    /// Resolves one axis to Klipper's conventional G-code letter.
    pub(super) fn axis_letter(axis: Axis) -> char {
        match axis {
            Axis::X => 'X',
            Axis::Y => 'Y',
            Axis::Z => 'Z',
            Axis::A => 'A',
            Axis::B => 'B',
            Axis::C => 'C',
        }
    }

    /// Sends an incremental absolute-safe jog script with G90 restored on every path.
    pub(super) async fn incremental_jog(
        &self,
        session: &mut Session,
        request: JogRequest,
    ) -> CncResult<()> {
        if ![-1, 1].contains(&request.direction)
            || !request.distance.is_finite()
            || request.distance <= 0.0
        {
            return Err(ControllerError::new(
                "bad_jog",
                "invalid Klipper jog request",
            ));
        }
        let feed = request.feed_rate.unwrap_or(100.0);
        if !feed.is_finite() || feed <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }
        let axis = Self::axis_letter(request.axis);
        let signed_distance = request.distance * f64::from(request.direction);
        self.script(
            session,
            &format!("G91\nG1 {axis}{signed_distance:.4} F{feed:.3}\nG90"),
        )
        .await
    }

    /// Executes the matching macro when present, otherwise sets the configured PWM pin.
    pub(super) async fn spindle(
        &self,
        session: &mut Session,
        command: SpindleCommand,
    ) -> CncResult<()> {
        if !command.rpm.is_finite() || command.rpm < 0.0 {
            return Err(ControllerError::new(
                "bad_spindle",
                "power must be finite and non-negative",
            ));
        }
        let macro_name = match command.direction {
            SpindleDirection::Clockwise => "M3",
            SpindleDirection::CounterClockwise => "M4",
            SpindleDirection::Stopped => "M5",
        };
        if session
            .object_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&format!("gcode_macro {macro_name}")))
        {
            self.script(session, &format!("{macro_name} S{}", command.rpm))
                .await?;
        } else if command.direction == SpindleDirection::Stopped {
            self.laser_off(session).await?;
        } else if let Some(pin) = &session.laser_pin {
            self.script(session, &format!("SET_PIN PIN={pin} VALUE={}", command.rpm))
                .await?;
        } else {
            return Err(ControllerError::new(
                "unsupported",
                "Klipper has no M3/M4 macro and no configured PWM output",
            ));
        }
        session.spindle_command = command;
        Ok(())
    }

    /// Sends the next jog segment only after Moonraker reports the toolhead queue is empty.
    pub(super) async fn pump_continuous_jog(&self, session: &mut Session) -> CncResult<()> {
        if session.continuous_jog.is_none() {
            return Ok(());
        }
        // `idle_timeout.state` describes the printer's idle timer, not whether
        // the toolhead has finished queued motion. `print_stats.state` becomes
        // `standby` after the prior G-code script drains from the toolhead queue.
        let toolhead_idle = session
            .objects
            .get("print_stats")
            .and_then(|value| value.get("state"))
            .and_then(Value::as_str)
            .is_some_and(|state| state.eq_ignore_ascii_case("standby"));
        if toolhead_idle {
            self.send_next_jog_chunk(session).await?;
        }
        Ok(())
    }

    /// Queries status fields and refreshes the subscription cache synchronously.
    pub(super) async fn refresh_status(
        &self,
        session: &mut Session,
    ) -> CncResult<crate::MachineStatus> {
        let response = self.rpc(session, "printer.objects.query", json!({"objects": super::status::subscription_objects(&session.object_names).into_iter().map(|n| (n, Value::Null)).collect::<serde_json::Map<_, _>>() })).await?;
        if let Some(status) = response.get("status").and_then(Value::as_object) {
            for (key, value) in status {
                session.objects.insert(key.clone(), value.clone());
            }
        }
        let ready = self
            .rpc(session, "server.info", json!({}))
            .await
            .ok()
            .and_then(|info| {
                info["klippy_state"]
                    .as_str()
                    .map(|s| s.eq_ignore_ascii_case("ready"))
            })
            .unwrap_or(false);
        let snapshot = super::status::map_status(session, ready)?;
        if ready {
            self.pump_continuous_jog(session).await?;
        }
        self.publish_status(snapshot.clone());
        Ok(snapshot)
    }
}
