//! Loads and validates axis and spindle limits from the emulator configuration.
use crate::cnc_api::{
    Axis, AxisConfiguration, AxisUnits, MachineAxis, MachineConfiguration, SpindleConfiguration,
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

#[derive(Debug, Clone, Deserialize)]
pub(super) struct SpindleConfig {
    pub(super) min_rpm: f64,
    pub(super) max_rpm: f64,
    pub(super) acceleration_rpm_per_sec: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct EmulatorConfig {
    pub(super) axes: AxesConfig,
    pub(super) spindle: SpindleConfig,
}

impl EmulatorConfig {
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

        MachineConfiguration {
            axes,
            spindle: SpindleConfiguration {
                min_rpm: Some(self.spindle.min_rpm),
                max_rpm: Some(self.spindle.max_rpm),
            },
            settings: Default::default(),
        }
    }

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

        if !self.spindle.min_rpm.is_finite()
            || self.spindle.min_rpm < 0.0
            || !self.spindle.max_rpm.is_finite()
            || self.spindle.max_rpm <= 0.0
            || self.spindle.min_rpm > self.spindle.max_rpm
            || !self.spindle.acceleration_rpm_per_sec.is_finite()
            || self.spindle.acceleration_rpm_per_sec <= 0.0
        {
            return Err("invalid spindle configuration".into());
        }
        Ok(())
    }
}

pub(super) fn load_config(path: &Path) -> Result<EmulatorConfig, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let config: EmulatorConfig = serde_yaml_ng::from_str(&content)
        .map_err(|e| format!("invalid YAML {}: {e}", path.display()))?;
    config.validate()?;
    Ok(config)
}
