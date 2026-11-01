//! Converts Moonraker's parsed `printer.cfg` settings into CNC configuration data.

use crate::{
    AxisConfiguration, AxisUnits, MachineAxis, MachineConfiguration, SpindleConfiguration,
};
use serde_json::Value;

impl super::KlipperAdapter {
    /// Builds machine limits and spindle capabilities from configfile settings.
    pub(super) fn configuration_from_settings(settings: &Value) -> MachineConfiguration {
        let printer = settings.get("printer");
        let kinematics = printer
            .and_then(|p| p.get("kinematics"))
            .and_then(Value::as_str)
            .unwrap_or("cartesian");
        let definitions = [
            ("stepper_x", MachineAxis::X, AxisUnits::Linear, "x"),
            ("stepper_y", MachineAxis::Y, AxisUnits::Linear, "y"),
            ("stepper_z", MachineAxis::Z, AxisUnits::Linear, "z"),
            ("stepper_a", MachineAxis::A, AxisUnits::Angular, "a"),
            ("stepper_b", MachineAxis::B, AxisUnits::Angular, "b"),
            ("stepper_c", MachineAxis::C, AxisUnits::Angular, "c"),
        ];
        let axes = definitions
            .into_iter()
            .filter_map(|(section, axis, units, _letter)| {
                let values = settings.get(section)?.as_object()?;
                let min = number(values.get("position_min"));
                let max = number(values.get("position_max"));
                let travel = if kinematics.eq_ignore_ascii_case("delta") {
                    None
                } else {
                    match (min, max) {
                        (Some(min), Some(max)) if max > min => Some(max - min),
                        _ => None,
                    }
                };
                Some(AxisConfiguration {
                    axis,
                    units,
                    max_travel: travel,
                    max_velocity: number(printer.and_then(|p| p.get("max_velocity")))
                        .map(|v| v * 60.0),
                    acceleration: number(printer.and_then(|p| p.get("max_accel"))),
                    steps_per_unit: None,
                })
            })
            .collect();
        let laser_pin = detect_laser_pin(settings);
        MachineConfiguration {
            axes,
            spindle: SpindleConfiguration {
                min_rpm: None,
                max_rpm: None,
                mode: if laser_pin.is_some() {
                    crate::SpindleMode::LaserDynamic
                } else {
                    crate::SpindleMode::Standard
                },
                laser: laser_pin.map(|_| crate::LaserConfiguration {
                    min_power: 0.0,
                    max_power: 1.0,
                    pwm_frequency_hz: None,
                    dynamic_power_supported: settings.get("pwm_tool").is_some(),
                    auto_shutoff_on_pause: true,
                }),
            },
            settings: std::collections::HashMap::new(),
        }
    }
}

/// Detects exactly one configured PWM or output pin as a laser channel.
pub(super) fn detect_laser_pin(settings: &Value) -> Option<String> {
    ["pwm_tool", "output_pin"].into_iter().find_map(|section| {
        settings.get(section)?.as_object().and_then(|pins| {
            (pins.len() == 1)
                .then(|| pins.keys().next().cloned())
                .flatten()
        })
    })
}

/// Parses finite numeric settings, accepting Moonraker strings as well as JSON numbers.
fn number(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    let result = value.as_f64().or_else(|| value.as_str()?.parse().ok())?;
    result.is_finite().then_some(result)
}
