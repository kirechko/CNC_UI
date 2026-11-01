//! Loads and validates axis and spindle limits from the emulator configuration.
use crate::cnc_api::{
    Axis, AxisConfiguration, AxisUnits, LaserConfiguration, MachineAxis, MachineConfiguration,
    SpindleConfiguration, SpindleMode,
};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct AxisLimits {
    pub(super) min_mm: f64,
    pub(super) max_mm: f64,
    pub(super) max_velocity_mm_min: f64,
    pub(super) acceleration_mm_s2: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct AngularAxisLimits {
    pub(super) min_degrees: f64,
    pub(super) max_degrees: f64,
    pub(super) max_velocity_degrees_min: f64,
    pub(super) acceleration_degrees_s2: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct AxesConfig {
    pub(super) x: AxisLimits,
    pub(super) y: AxisLimits,
    #[serde(default)]
    pub(super) z: Option<AxisLimits>,
    #[serde(default)]
    pub(super) a: Option<AngularAxisLimits>,
    #[serde(default)]
    pub(super) b: Option<AngularAxisLimits>,
}

/// Output-channel limits and optional laser behavior loaded from YAML.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct SpindleConfig {
    /// Conventional spindle minimum RPM (legacy range kept for backward-compatible YAML).
    pub(super) min_rpm: f64,
    /// Conventional spindle maximum RPM (laser range comes from `laser.max_power`).
    pub(super) max_rpm: f64,
    /// Simulated ramp rate for conventional spindle mode.
    pub(super) acceleration_rpm_per_sec: f64,
    /// Machine output interpretation, defaulting to a conventional spindle.
    #[serde(default)]
    pub(super) mode: SpindleMode,
    /// Optional native laser S-word limits and safety preferences.
    #[serde(default)]
    pub(super) laser: Option<LaserConfiguration>,
}

/// Top-level emulator setup loaded from the user-selected YAML file.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct EmulatorConfig {
    pub(super) axes: AxesConfig,
    pub(super) spindle: SpindleConfig,
}

impl EmulatorConfig {
    /// Exposes configured axes and output-channel limits through the shared CNC API model.
    pub(super) fn machine_configuration(&self) -> MachineConfiguration {
        let linear = |axis, limits: &AxisLimits| AxisConfiguration {
            axis,
            units: AxisUnits::Linear,
            max_travel: Some(limits.max_mm - limits.min_mm),
            max_velocity: Some(limits.max_velocity_mm_min),
            acceleration: Some(limits.acceleration_mm_s2),
            steps_per_unit: None,
        };
        let angular = |axis, limits: &AngularAxisLimits| AxisConfiguration {
            axis,
            units: AxisUnits::Angular,
            max_travel: Some(limits.max_degrees - limits.min_degrees),
            max_velocity: Some(limits.max_velocity_degrees_min),
            acceleration: Some(limits.acceleration_degrees_s2),
            steps_per_unit: None,
        };

        let mut axes = vec![
            linear(MachineAxis::X, &self.axes.x),
            linear(MachineAxis::Y, &self.axes.y),
        ];
        if let Some(limits) = &self.axes.z {
            axes.push(linear(MachineAxis::Z, limits));
        }
        if let Some(limits) = &self.axes.a {
            axes.push(angular(MachineAxis::A, limits));
        }
        if let Some(limits) = &self.axes.b {
            axes.push(angular(MachineAxis::B, limits));
        }

        let (minimum, maximum) = self
            .spindle
            .laser
            .map(|laser| (laser.min_power, laser.max_power))
            .unwrap_or((self.spindle.min_rpm, self.spindle.max_rpm));
        // Report the configured output range in the shared speed slots; `laser`
        // separately labels those values as native power rather than RPM.
        MachineConfiguration {
            axes,
            spindle: SpindleConfiguration {
                min_rpm: Some(minimum),
                max_rpm: Some(maximum),
                mode: self.spindle.mode,
                laser: self.spindle.laser,
            },
            settings: Default::default(),
        }
    }

    /// Returns axis identifiers that have valid travel and motion settings.
    pub(super) fn configured_axes(&self) -> Vec<Axis> {
        let mut axes = vec![Axis::X, Axis::Y];
        if self.axes.z.is_some() {
            axes.push(Axis::Z);
        }
        if self.axes.a.is_some() {
            axes.push(Axis::A);
        }
        if self.axes.b.is_some() {
            axes.push(Axis::B);
        }
        axes
    }

    /// Returns min/max travel, velocity, and acceleration for a configured axis.
    pub(super) fn axis_limits(&self, axis: Axis) -> (f64, f64, f64, f64) {
        match axis {
            Axis::X => (
                self.axes.x.min_mm,
                self.axes.x.max_mm,
                self.axes.x.max_velocity_mm_min,
                self.axes.x.acceleration_mm_s2,
            ),
            Axis::Y => (
                self.axes.y.min_mm,
                self.axes.y.max_mm,
                self.axes.y.max_velocity_mm_min,
                self.axes.y.acceleration_mm_s2,
            ),
            Axis::Z => self.axes.z.as_ref().map_or((0.0, 0.0, 0.0, 0.0), |limits| {
                (
                    limits.min_mm,
                    limits.max_mm,
                    limits.max_velocity_mm_min,
                    limits.acceleration_mm_s2,
                )
            }),
            Axis::A => self.axes.a.as_ref().map_or((0.0, 0.0, 0.0, 0.0), |limits| {
                (
                    limits.min_degrees,
                    limits.max_degrees,
                    limits.max_velocity_degrees_min,
                    limits.acceleration_degrees_s2,
                )
            }),
            Axis::B => self.axes.b.as_ref().map_or((0.0, 0.0, 0.0, 0.0), |limits| {
                (
                    limits.min_degrees,
                    limits.max_degrees,
                    limits.max_velocity_degrees_min,
                    limits.acceleration_degrees_s2,
                )
            }),
            Axis::C => (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// Rejects non-finite or inconsistent axis, spindle, and laser profile values.
    pub(super) fn validate(&self) -> Result<(), String> {
        for (name, a) in [
            ("X", &self.axes.x),
            ("Y", &self.axes.y),
            ("Z", self.axes.z.as_ref().unwrap_or(&self.axes.x)),
        ] {
            if !a.min_mm.is_finite()
                || !a.max_mm.is_finite()
                || a.min_mm >= a.max_mm
                || !a.max_velocity_mm_min.is_finite()
                || a.max_velocity_mm_min <= 0.0
                || !a.acceleration_mm_s2.is_finite()
                || a.acceleration_mm_s2 <= 0.0
            {
                return Err(format!("invalid configuration for axis {name}"));
            }
        }
        for (name, axis) in [
            (
                "Z",
                self.axes.z.as_ref().map(|limits| {
                    (
                        limits.min_mm,
                        limits.max_mm,
                        limits.max_velocity_mm_min,
                        limits.acceleration_mm_s2,
                    )
                }),
            ),
            (
                "A",
                self.axes.a.as_ref().map(|limits| {
                    (
                        limits.min_degrees,
                        limits.max_degrees,
                        limits.max_velocity_degrees_min,
                        limits.acceleration_degrees_s2,
                    )
                }),
            ),
            (
                "B",
                self.axes.b.as_ref().map(|limits| {
                    (
                        limits.min_degrees,
                        limits.max_degrees,
                        limits.max_velocity_degrees_min,
                        limits.acceleration_degrees_s2,
                    )
                }),
            ),
        ] {
            if let Some((minimum, maximum, velocity, acceleration)) = axis {
                if !minimum.is_finite()
                    || !maximum.is_finite()
                    || minimum >= maximum
                    || !velocity.is_finite()
                    || velocity <= 0.0
                    || !acceleration.is_finite()
                    || acceleration <= 0.0
                {
                    return Err(format!("invalid configuration for axis {name}"));
                }
            }
        }

        let output_minimum = self
            .spindle
            .laser
            .map_or(self.spindle.min_rpm, |laser| laser.min_power);
        let output_maximum = self
            .spindle
            .laser
            .map_or(self.spindle.max_rpm, |laser| laser.max_power);
        if !self.spindle.min_rpm.is_finite()
            || self.spindle.min_rpm < 0.0
            || !self.spindle.max_rpm.is_finite()
            || self.spindle.max_rpm <= 0.0
            || self.spindle.min_rpm > self.spindle.max_rpm
            || !output_minimum.is_finite()
            || output_minimum < 0.0
            || !output_maximum.is_finite()
            || output_maximum <= output_minimum
            || !self.spindle.acceleration_rpm_per_sec.is_finite()
            || self.spindle.acceleration_rpm_per_sec <= 0.0
        {
            return Err("invalid spindle configuration".into());
        }
        if let Some(laser) = self.spindle.laser {
            if self.spindle.mode == SpindleMode::Standard
                || laser
                    .pwm_frequency_hz
                    .is_some_and(|frequency| !frequency.is_finite() || frequency <= 0.0)
            {
                return Err("invalid laser configuration or spindle mode".into());
            }
        } else if self.spindle.mode != SpindleMode::Standard {
            return Err("laser mode requires a laser configuration".into());
        }
        Ok(())
    }
}

/// Reads, deserializes, and validates one emulator YAML configuration.
pub(super) fn load_config(path: &Path) -> Result<EmulatorConfig, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let config: EmulatorConfig = serde_yaml_ng::from_str(&content)
        .map_err(|e| format!("invalid YAML {}: {e}", path.display()))?;
    config.validate()?;
    Ok(config)
}
