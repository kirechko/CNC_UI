//! Focused tests for source-level preflight travel-limit analysis.

use super::check_profile_bounds;
use crate::web_server::machine_profiles::{MachineProfile, Units as ProfileUnits};

/// Builds a valid profile with known X bounds and unknown Y/Z travel.
fn profile() -> MachineProfile {
    MachineProfile {
        id: "test-profile".into(),
        name: "Test mill".into(),
        machine_type: crate::web_server::machine_profiles::MachineType::Mill,
        units: ProfileUnits::Millimeters,
        travel: crate::web_server::machine_profiles::AxisTravel {
            x_min: Some(0.0),
            x_max: Some(10.0),
            ..Default::default()
        },
        speeds: crate::web_server::machine_profiles::AxisSpeeds::default(),
        power: None,
        source_priorities: crate::web_server::machine_profiles::SourcePriorities::default(),
        tool_reference: None,
        extensions: serde_json::Value::Null,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

/// Reports effective configured bound violations as blockers and missing bounds as warnings.
#[test]
fn profile_bounds_block_only_known_outside_axes() {
    let path = rosetta::io::gcode::parse_toolpath("G21\nG90\nG0 X12 Y2 Z0\n").unwrap();
    let profile = profile();
    let resolved =
        crate::web_server::machine_profiles::resolution::resolve(Some(&profile), None, &[]);
    let mut findings = Vec::new();
    check_profile_bounds(
        &path,
        Some(&profile.name),
        ProfileUnits::Millimeters,
        &resolved.axes,
        &mut findings,
    );
    assert!(findings
        .iter()
        .any(|finding| finding.code == "travel_limit_exceeded" && finding.line == Some(3)));
    assert_eq!(
        findings
            .iter()
            .filter(|finding| finding.code == "travel_limit_unknown")
            .count(),
        2
    );
}
