 //! Loads and validates axis and spindle limits from the emulator configuration.
use crate::cnc_api::{
    Axis, AxisConfiguration, AxisUnits, MachineAxis, MachineConfiguration,
    SpindleConfiguration,
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
pub(super) struct AxesConfig {
    pub(super) x: AxisLimits,
    pub(super) y: AxisLimits,
    pub(super) z: AxisLimits,
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
        let axis_configuration = |axis, limits: &AxisLimits| AxisConfiguration {
            axis,
            units: AxisUnits::Linear,
            max_travel: Some(limits.max_mm - limits.min_mm),
        };

        MachineConfiguration {
            axes: vec![
                axis_configuration(MachineAxis::X, &self.axes.x),
                axis_configuration(MachineAxis::Y, &self.axes.y),
                axis_configuration(MachineAxis::Z, &self.axes.z),
            ],
            spindle: SpindleConfiguration {
                min_rpm: Some(self.spindle.min_rpm),
                max_rpm: Some(self.spindle.max_rpm),
            },
        }
    }

    pub(super) fn axis(&self, axis: Axis) -> &AxisLimits {
        match axis {
            Axis::X => &self.axes.x,
            Axis::Y => &self.axes.y,
            Axis::Z => &self.axes.z,
        }
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        for (name, a) in [
            ("X", &self.axes.x),
            ("Y", &self.axes.y),
            ("Z", &self.axes.z),
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
