//! Jog facade of [`CNCController`]: feed-rate settings, step and continuous
//! moves for single axes and the XY plane.

use super::{CNCController, Command};
use crate::cnc_api::Axis;

// Jog entry points are consumed by the Tauri commands; the axum binary does
// not reach every method, hence the deliberate `#[allow(dead_code)]`.
#[allow(dead_code)]
impl CNCController {
    pub fn set_xy_jog_speed(&self, speed: f64) {
        if speed.is_finite() && speed > 0.0 {
            *self.xy_jog_speed.lock().unwrap() = speed;
        }
    }

    pub fn set_z_jog_speed(&self, speed: f64) {
        if speed.is_finite() && speed > 0.0 {
            *self.z_jog_speed.lock().unwrap() = speed;
        }
    }

    /// Sets the shared jog feed for the rotary axes A, B and C.
    /// Non-finite or non-positive values are ignored.
    pub fn set_rotary_jog_speed(&self, speed: f64) {
        if speed.is_finite() && speed > 0.0 {
            *self.rotary_jog_speed.lock().unwrap() = speed;
        }
    }

    pub fn set_jog_step(&self, step: f64) {
        if step.is_finite() && step > 0.0 {
            self.status.lock().unwrap().jog_step = step;
        }
    }

    /// Diagonal step jog: relative G1 move in XY followed by absolute mode restore.
    pub fn jog_xy_step(&self, x_direction: i8, y_direction: i8, step: f64) {
        if !step.is_finite()
            || step <= 0.0
            || ![-1, 1].contains(&x_direction)
            || ![-1, 1].contains(&y_direction)
        {
            eprintln!("[UI] Invalid XY jog request");
            return;
        }

        let feed = *self.xy_jog_speed.lock().unwrap();
        crate::message_log::record(
            "info",
            "Jog",
            &format!(
                "Step jog XY: X {x_direction:+}, Y {y_direction:+}, distance {step:.3} mm, feed {feed:.3} mm/min",
            ),
        );
        self.enqueue(Command::Mdi(format!(
            "G91 G1 X{:.3} Y{:.3} F{feed:.0}",
            step * f64::from(x_direction),
            step * f64::from(y_direction),
        )));
        self.enqueue(Command::Mdi("G90".into()));
    }

    /// Returns the configured jog feed for the given axis.
    /// X and Y share the XY feed, Z has its own feed, and A, B and C share the rotary feed.
    pub(super) fn feed_for_axis(&self, axis: Axis) -> f64 {
        match axis {
            Axis::Z => *self.z_jog_speed.lock().unwrap(),
            Axis::X | Axis::Y => *self.xy_jog_speed.lock().unwrap(),
            Axis::A | Axis::B | Axis::C => *self.rotary_jog_speed.lock().unwrap(),
        }
    }

    /// Step jog of a single axis, letter-parsed (X/Y/Z/A/B/C).
    pub fn jog_axis_step(&self, axis: char, direction: i8, step: f64) {
        let Some(axis) = parse_axis(axis) else {
            eprintln!("[UI] Unsupported jog axis: {axis}");
            return;
        };

        if !step.is_finite() || step <= 0.0 || ![-1, 1].contains(&direction) {
            eprintln!("[UI] Invalid jog request");
            return;
        }

        let feed_rate = self.feed_for_axis(axis);

        crate::message_log::record(
            "info",
            "Jog",
            &format!(
                "Step jog {:?}: direction {direction:+}, distance {step:.3}, feed {feed_rate:.3}",
                axis,
            ),
        );
        self.enqueue(Command::JogIncrement(crate::cnc_api::JogRequest {
            axis,
            direction,
            distance: step,
            feed_rate: Some(feed_rate),
        }));
    }

    /// Step jog using the currently stored jog step.
    pub fn jog_axis(&self, axis: char, direction: i8) {
        let step = self.status.lock().unwrap().jog_step;
        self.jog_axis_step(axis, direction, step);
    }

    /// Continuous single-axis jog; `direction` 0 stops the axis.
    pub fn jog_axis_continuous(&self, axis: char, direction: i8) {
        let Some(axis) = parse_axis(axis) else {
            eprintln!("[UI] Unsupported continuous jog axis: {axis}");
            return;
        };

        if ![-1, 0, 1].contains(&direction) {
            eprintln!("[UI] Invalid continuous jog direction: {direction}");
            return;
        }

        let feed_rate = self.feed_for_axis(axis);
        self.enqueue(Command::JogContinuous(axis, direction, feed_rate));
    }

    /// Continuous XY (diagonal-capable) jog; both zero means stop.
    pub fn jog_xy_continuous(&self, x_direction: i8, y_direction: i8) {
        if ![-1, 0, 1].contains(&x_direction) || ![-1, 0, 1].contains(&y_direction) {
            eprintln!("[UI] Invalid continuous XY jog direction");
            return;
        }

        let feed_rate = *self.xy_jog_speed.lock().unwrap();
        self.enqueue(Command::JogContinuousXY(x_direction, y_direction, feed_rate));
    }

    pub fn cancel_jog(&self) {
        self.enqueue(Command::JogCancel);
    }
}

/// Maps a jog axis letter to the API enum; unknown letters return `None`
/// (the callers log the rejection because the message differs per method).
fn parse_axis(axis: char) -> Option<Axis> {
    match axis.to_ascii_uppercase() {
        'X' => Some(Axis::X),
        'Y' => Some(Axis::Y),
        'Z' => Some(Axis::Z),
        'A' => Some(Axis::A),
        'B' => Some(Axis::B),
        'C' => Some(Axis::C),
        _ => None,
    }
}
