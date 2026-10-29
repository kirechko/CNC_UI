//! Crate-root unit tests (flat module, enabled by `#[cfg(test)] mod tests;` in
//! `lib.rs`): adapter trait compliance, wire-format contract stability and
//! backwards-compatible `Position` deserialization.

use crate::adapters::{
    debug::{FluidNcAdapter, LinuxCncAdapter},
    emulator::EmAdapter,
    grbl::GrblAdapter,
};
use crate::{api::CncApi, Axis, CoordinateSystem, JogRequest, Position};
use std::path::Path;

/// Compile-time check that a type implements the shared control API.
fn assert_core_adapter<T: CncApi>() {}

#[test]
fn migrated_adapters_implement_the_core_api() {
    assert_core_adapter::<LinuxCncAdapter>();
    assert_core_adapter::<FluidNcAdapter>();
    assert_core_adapter::<EmAdapter>();
    assert_core_adapter::<GrblAdapter>();
}

#[test]
fn core_types_keep_their_serialized_contract() {
    let request = JogRequest {
        axis: Axis::X,
        direction: -1,
        distance: 0.1,
        feed_rate: Some(1200.0),
    };
    let value = serde_json::to_value(request).unwrap();

    assert_eq!(
        value,
        serde_json::json!({
            "axis": "X",
            "direction": -1,
            "distance": 0.1,
            "feed_rate": 1200.0
        })
    );
    assert_eq!(
        serde_json::to_value(CoordinateSystem::G59_1).unwrap(),
        "g59_1"
    );
}

#[test]
fn position_deserialization_defaults_rotary_axes_for_older_payloads() {
    let position: Position =
        serde_json::from_value(serde_json::json!({ "x": 1.0, "y": 2.0, "z": 3.0 })).unwrap();

    assert_eq!(position.a, 0.0);
    assert_eq!(position.b, 0.0);
}

#[allow(dead_code)]
fn assert_load_program_accepts_paths(_: &Path) {}
