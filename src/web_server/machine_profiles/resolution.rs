//! Read-only resolution of user-authored machine metadata against cached controller configuration.

use super::{MachineProfile, MachineType, SourcePriority, Units};
use crate::cnc_api::{MachineAxis, MachineConfiguration};
use crate::web_server::tool_library::ToolRecord;
use cnc_core::SpindleMode;
use serde::Serialize;

/// Provenance label identifying which metadata source supplied a resolved value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataSource {
    /// Value was explicitly authored in the selected local machine profile.
    Profile,
    /// Value was observed in the adapter's cached controller configuration.
    Controller,
    /// No known source supplied the value.
    Unknown,
}

impl MetadataSource {
    /// Combines per-bound provenance into a single legacy field-level provenance.
    fn max_source(self, other: Self) -> Self {
        match (self, other) {
            (Self::Profile, _) | (_, Self::Profile) => Self::Profile,
            (Self::Controller, _) | (_, Self::Controller) => Self::Controller,
            _ => Self::Unknown,
        }
    }
}

/// A generic effective metadata field paired with the provenance that supplied its value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedValue<T> {
    /// Effective value selected according to the profile's source preference and available data.
    pub value: T,
    /// Source that supplied the effective value, or unknown when no source had a value.
    pub source: MetadataSource,
}

/// One axis's resolved travel and speed metadata, with explicit provenance and conflicts.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAxis {
    /// Linear axis symbol represented by this result.
    pub axis: String,
    /// User-authored travel interval in the profile's declared units.
    pub profile_travel: (Option<f64>, Option<f64>),
    /// User-authored minimum linear-axis travel bound.
    pub profile_min_travel: Option<f64>,
    /// Cached controller maximum travel mapped to the positive bound; controller config has no zero origin.
    pub controller_min_travel: Option<f64>,
    /// Controller-reported maximum travel converted to profile units when possible.
    pub controller_max_travel: Option<f64>,
    /// User-authored maximum speed in profile units per minute.
    pub profile_max_speed: Option<f64>,
    /// Controller-reported maximum speed converted to profile units per minute.
    pub controller_max_speed: Option<f64>,
    /// Effective minimum travel bound selected from profile/controller metadata.
    pub effective_travel_min: ResolvedValue<Option<f64>>,
    /// Effective maximum travel bound selected from profile/controller metadata.
    pub effective_travel_max: ResolvedValue<Option<f64>>,
    /// Effective maximum speed selected from profile/controller metadata.
    pub effective_max_speed: ResolvedValue<Option<f64>>,
    /// Provenance of configured travel bounds, retained for compatibility.
    pub travel_source: MetadataSource,
    /// Provenance of configured maximum speed, retained for compatibility.
    pub speed_source: MetadataSource,
    /// Source disagreements that require operator awareness.
    pub conflicts: Vec<String>,
}

/// Immutable merged configuration snapshot; resolving it never applies profile values to the machine.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedConfiguration {
    /// Selected local profile id, if one was provided.
    pub profile_id: Option<String>,
    /// Effective machine category selected from profile metadata and controller output mode.
    pub machine_type: ResolvedValue<Option<MachineType>>,
    /// Effective supported linear axes, represented by the X/Y/Z axis symbols, when known.
    pub effective_axes: ResolvedValue<Option<Vec<String>>>,
    /// Effective dimensional units, defaulting to millimeters without a profile.
    pub effective_units: ResolvedValue<Units>,
    /// Units used for profile dimensions, retained as a convenient compatibility field.
    pub units: Units,
    /// Resolved linear axes X/Y/Z with per-value provenance.
    pub axes: Vec<ResolvedAxis>,
    /// Effective process power range selected from profile or cached controller configuration.
    pub process_power: ResolvedValue<Option<super::PowerProfile>>,
    /// Profile-authored power limits; this is not live machine telemetry.
    pub profile_power: Option<super::PowerProfile>,
    /// Optional stable local tool record id referenced by the profile.
    pub tool_reference: Option<String>,
    /// Whether the referenced local tool record exists.
    pub tool_reference_found: Option<bool>,
    /// Combined source conflicts for convenient snapshot-level inspection.
    pub conflicts: Vec<String>,
}

/// Selects the preferred value when present and transparently falls back to the alternate source.
fn choose<T>(
    profile_value: Option<T>,
    controller_value: Option<T>,
    priority: SourcePriority,
) -> ResolvedValue<Option<T>> {
    let (value, source) = match priority {
        SourcePriority::ProfileFirst => profile_value
            .map(|value| (Some(value), MetadataSource::Profile))
            .or_else(|| controller_value.map(|value| (Some(value), MetadataSource::Controller))),
        SourcePriority::ControllerFirst => controller_value
            .map(|value| (Some(value), MetadataSource::Controller))
            .or_else(|| profile_value.map(|value| (Some(value), MetadataSource::Profile))),
    }
    .unwrap_or((None, MetadataSource::Unknown));
    ResolvedValue { value, source }
}

/// Resolves compatible metadata from profile and cached controller configuration without machine I/O.
pub fn resolve(
    profile: Option<&MachineProfile>,
    controller: Option<&MachineConfiguration>,
    tools: &[ToolRecord],
) -> ResolvedConfiguration {
    let priorities = profile.map_or_else(Default::default, |item| item.source_priorities);
    // Controller configuration has no declared dimensional units, so only profile units can supply this field.
    let selected_units = choose(profile.map(|item| item.units), None, priorities.units);
    let units = selected_units.value.unwrap_or(Units::Millimeters);
    let effective_units = ResolvedValue {
        value: units,
        source: selected_units.source,
    };
    let to_profile_units = match units {
        Units::Millimeters => 1.0,
        Units::Inches => 1.0 / 25.4,
    };
    let axis_labels = [MachineAxis::X, MachineAxis::Y, MachineAxis::Z];
    let controller_axes = controller.map_or_else(Vec::new, |configuration| {
        configuration
            .axes
            .iter()
            .filter(|item| axis_labels.contains(&item.axis))
            .map(|item| format!("{:?}", item.axis))
            .collect()
    });
    let profile_axes = profile.map(|_| vec!["X".to_string(), "Y".to_string(), "Z".to_string()]);
    let effective_axes: ResolvedValue<Option<Vec<String>>> = choose(
        profile_axes,
        (!controller_axes.is_empty()).then_some(controller_axes),
        priorities.axes,
    );
    let axes = [("X", MachineAxis::X), ("Y", MachineAxis::Y), ("Z", MachineAxis::Z)]
        .into_iter()
        .map(|(label, axis)| {
            let controller_axis = controller.and_then(|configuration| configuration.axes.iter().find(|candidate| candidate.axis == axis));
            let profile_travel = profile.map_or((None, None), |item| match label { "X" => (item.travel.x_min, item.travel.x_max), "Y" => (item.travel.y_min, item.travel.y_max), _ => (item.travel.z_min, item.travel.z_max) });
            let profile_min_travel = profile_travel.0;
            let controller_min_travel = None;
            let controller_max_travel = controller_axis.and_then(|item| item.max_travel).map(|value| value * to_profile_units);
            let profile_max_speed = profile.and_then(|item| match label { "X" => item.speeds.x, "Y" => item.speeds.y, _ => item.speeds.z });
            let controller_max_speed = controller_axis.and_then(|item| item.max_velocity).map(|value| value * to_profile_units);
            let effective_travel_min = choose(profile_travel.0, controller_min_travel, priorities.travel);
            let mut effective_travel_max = choose(profile_travel.1, controller_max_travel, priorities.travel);
            let mut effective_max_speed = choose(profile_max_speed, controller_max_speed, priorities.speeds);
            if priorities.travel == SourcePriority::ControllerFirst && profile.is_some() {
                if let Some(controller_max) = controller_max_travel {
                    effective_travel_max = ResolvedValue { value: Some(controller_max), source: MetadataSource::Controller };
                } else if let Some(profile_max) = profile_travel.1 {
                    effective_travel_max = ResolvedValue { value: Some(profile_max), source: MetadataSource::Profile };
                }
            }
            if priorities.speeds == SourcePriority::ControllerFirst {
                if let Some(controller_speed) = controller_max_speed {
                    effective_max_speed = ResolvedValue { value: Some(controller_speed), source: MetadataSource::Controller };
                } else if let Some(profile_speed) = profile_max_speed {
                    effective_max_speed = ResolvedValue { value: Some(profile_speed), source: MetadataSource::Profile };
                }
            }
            let mut conflicts = Vec::new();
            if let (Some(profile_max), Some(controller_max)) = (profile_travel.1, controller_max_travel) {
                if (profile_max - controller_max).abs() > 0.01 { conflicts.push(format!("{label} profile maximum travel {profile_max} differs from controller maximum {controller_max}")); }
            }
            if let (Some(profile_speed), Some(controller_speed)) = (profile_max_speed, controller_max_speed) {
                if (profile_speed - controller_speed).abs() > 0.01 { conflicts.push(format!("{label} profile maximum speed {profile_speed} differs from controller maximum {controller_speed}")); }
            }
            let travel_source = effective_travel_min.source.max_source(effective_travel_max.source);
            let speed_source = effective_max_speed.source;
            ResolvedAxis {
                axis: label.into(), profile_travel, profile_min_travel, controller_min_travel, controller_max_travel, profile_max_speed, controller_max_speed,
                effective_travel_min, effective_travel_max, effective_max_speed, travel_source, speed_source, conflicts,
            }
        })
        .collect::<Vec<_>>();
    let profile_power = profile.and_then(|item| item.power.clone());
    let controller_power = controller.and_then(|configuration| {
        let (kind, min, max) = if let Some(laser) = configuration.spindle.laser {
            (super::PowerKind::Laser, laser.min_power, laser.max_power)
        } else {
            (
                super::PowerKind::Spindle,
                configuration.spindle.min_rpm?,
                configuration.spindle.max_rpm?,
            )
        };
        Some(super::PowerProfile {
            kind,
            min: Some(min),
            max: Some(max),
        })
    });
    let process_power = choose(profile_power.clone(), controller_power, priorities.power);
    let controller_machine_type =
        controller.map(|configuration| match configuration.spindle.mode {
            SpindleMode::LaserConstant | SpindleMode::LaserDynamic => MachineType::Laser,
            SpindleMode::Standard => MachineType::Mill,
        });
    let machine_type = choose(
        profile.map(|item| item.machine_type),
        controller_machine_type,
        priorities.machine_type,
    );
    let tool_reference = profile.and_then(|item| item.tool_reference.clone());
    let tool_reference_found = tool_reference
        .as_ref()
        .map(|id| tools.iter().any(|tool| &tool.id == id));
    let conflicts = axes
        .iter()
        .flat_map(|axis| axis.conflicts.iter().cloned())
        .collect();
    ResolvedConfiguration {
        profile_id: profile.map(|item| item.id.clone()),
        machine_type,
        effective_axes,
        effective_units,
        units,
        axes,
        process_power,
        profile_power,
        tool_reference,
        tool_reference_found,
        conflicts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_server::machine_profiles::{
        AxisSpeeds, AxisTravel, MachineType, PowerKind, PowerProfile, SourcePriorities,
        SourcePriority,
    };

    /// Builds a minimal profile with a conflicting X maximum so resolver provenance can be asserted.
    fn test_profile() -> MachineProfile {
        MachineProfile {
            id: "profile".into(),
            name: "Profile".into(),
            machine_type: MachineType::Mill,
            units: Units::Millimeters,
            travel: AxisTravel {
                x_min: Some(0.0),
                x_max: Some(200.0),
                ..AxisTravel::default()
            },
            speeds: AxisSpeeds::default(),
            power: None,
            source_priorities: Default::default(),
            tool_reference: None,
            extensions: serde_json::Value::Null,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    /// Builds a controller snapshot with known travel, speed, spindle range, and all linear axes.
    fn test_controller() -> MachineConfiguration {
        MachineConfiguration {
            axes: [MachineAxis::X, MachineAxis::Y, MachineAxis::Z]
                .into_iter()
                .map(|axis| cnc_core::AxisConfiguration {
                    axis,
                    units: cnc_core::AxisUnits::Linear,
                    max_travel: Some(500.0),
                    max_velocity: Some(1200.0),
                    acceleration: None,
                    steps_per_unit: None,
                })
                .collect(),
            spindle: cnc_core::SpindleConfiguration {
                min_rpm: Some(100.0),
                max_rpm: Some(9000.0),
                mode: SpindleMode::Standard,
                laser: None,
            },
            settings: Default::default(),
        }
    }

    /// Confirms profile metadata provenance is retained when no live controller snapshot exists.
    #[test]
    fn resolver_retains_profile_provenance_without_controller_data() {
        let profile = test_profile();
        let snapshot = resolve(Some(&profile), None, &[]);
        assert_eq!(snapshot.axes[0].travel_source, MetadataSource::Profile);
        assert_eq!(snapshot.axes[0].controller_max_travel, None);
        assert!(snapshot.conflicts.is_empty());
    }

    /// Confirms controller-first choices fall back per field when the preferred source lacks a value.
    #[test]
    fn controller_first_priorities_fallback_per_field_and_keep_provenance() {
        let mut profile = test_profile();
        profile.source_priorities = SourcePriorities {
            machine_type: SourcePriority::ControllerFirst,
            axes: SourcePriority::ControllerFirst,
            units: SourcePriority::ControllerFirst,
            travel: SourcePriority::ControllerFirst,
            speeds: SourcePriority::ControllerFirst,
            power: SourcePriority::ControllerFirst,
        };
        profile.speeds.x = Some(800.0);
        profile.power = Some(PowerProfile {
            kind: PowerKind::Spindle,
            min: Some(0.0),
            max: Some(12000.0),
        });
        let snapshot = resolve(Some(&profile), Some(&test_controller()), &[]);

        assert_eq!(snapshot.machine_type.value, Some(MachineType::Mill));
        assert_eq!(snapshot.machine_type.source, MetadataSource::Controller);
        assert_eq!(snapshot.effective_units.value, Units::Millimeters);
        assert_eq!(snapshot.effective_units.source, MetadataSource::Profile);
        assert_eq!(snapshot.axes[0].effective_travel_min.value, Some(0.0));
        assert_eq!(
            snapshot.axes[0].effective_travel_min.source,
            MetadataSource::Profile
        );
        assert_eq!(snapshot.axes[0].effective_travel_max.value, Some(500.0));
        assert_eq!(
            snapshot.axes[0].effective_travel_max.source,
            MetadataSource::Controller
        );
        assert_eq!(snapshot.axes[0].effective_max_speed.value, Some(1200.0));
        assert_eq!(
            snapshot.axes[0].effective_max_speed.source,
            MetadataSource::Controller
        );
        assert_eq!(
            snapshot.process_power.value.as_ref().unwrap().max,
            Some(9000.0)
        );
        assert_eq!(snapshot.process_power.source, MetadataSource::Controller);
        assert_eq!(snapshot.effective_axes.source, MetadataSource::Controller);
    }

    /// Confirms missing controller data falls back to profile values and missing data stays unknown.
    #[test]
    fn preferred_controller_source_absence_falls_back_to_profile_or_unknown() {
        let mut profile = test_profile();
        profile.source_priorities.travel = SourcePriority::ControllerFirst;
        profile.source_priorities.speeds = SourcePriority::ControllerFirst;
        profile.speeds.x = Some(700.0);
        let snapshot = resolve(Some(&profile), None, &[]);

        assert_eq!(snapshot.axes[0].effective_travel_min.value, Some(0.0));
        assert_eq!(
            snapshot.axes[0].effective_travel_min.source,
            MetadataSource::Profile
        );
        assert_eq!(snapshot.axes[0].effective_max_speed.value, Some(700.0));
        assert_eq!(
            snapshot.axes[0].effective_max_speed.source,
            MetadataSource::Profile
        );
        assert_eq!(snapshot.axes[1].effective_travel_min.value, None);
        assert_eq!(
            snapshot.axes[1].effective_travel_min.source,
            MetadataSource::Unknown
        );
        assert_eq!(snapshot.process_power.value, None);
    }

    /// Confirms a profile source preference is honored when both controller and profile values exist.
    #[test]
    fn profile_first_priority_beats_controller_values() {
        let profile = test_profile();
        let snapshot = resolve(Some(&profile), Some(&test_controller()), &[]);
        assert_eq!(snapshot.axes[0].effective_travel_max.value, Some(200.0));
        assert_eq!(
            snapshot.axes[0].effective_travel_max.source,
            MetadataSource::Profile
        );
        assert_eq!(
            snapshot.axes[0].effective_max_speed.source,
            MetadataSource::Controller
        );
    }

    /// Confirms controller-only values populate effective snapshot fields without an active profile.
    #[test]
    fn controller_only_snapshot_is_available_without_active_profile() {
        let snapshot = resolve(None, Some(&test_controller()), &[]);
        assert_eq!(snapshot.machine_type.value, Some(MachineType::Mill));
        assert_eq!(snapshot.machine_type.source, MetadataSource::Controller);
        assert_eq!(snapshot.axes[0].effective_travel_max.value, Some(500.0));
        assert_eq!(
            snapshot.axes[0].effective_travel_max.source,
            MetadataSource::Controller
        );
        assert_eq!(snapshot.process_power.source, MetadataSource::Controller);
        assert_eq!(snapshot.effective_units.source, MetadataSource::Unknown);
    }
}
