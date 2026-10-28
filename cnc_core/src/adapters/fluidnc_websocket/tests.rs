//! Unit tests for the FluidNC WebSocket adapter: disconnected-startup
//! subscriptions, program normalization, wire-byte accounting and status
//! axis-count parsing. The module is flat so `use super::*` pulls the
//! parent module's private helpers into scope.

use super::*;
use crate::{machine_state, CncApi, MachineState};

#[test]
fn channel_api_starts_with_disconnected_snapshot_and_subscriptions() {
    let adapter = FluidNcWebSocketAdapter::connect("ws://127.0.0.1:81/")
        .expect("valid WebSocket URL should construct an adapter");

    let snapshot = adapter.last_status().expect("status snapshot is supported");
    assert!(!snapshot.connected);
    assert_eq!(machine_state(&snapshot), MachineState::Disconnected);
    assert!(adapter.subscribe_status().is_some());
    assert!(adapter.subscribe_events().is_some());
}

#[test]
fn program_normalization_matches_browser_executable_line_counting() {
    let source = concat!(
        "; full-line comment\n",
        "(parenthesized comment)\n",
        "G21 (inline comment) ; trailing comment\n",
        "  G1 X10 (more) Y20  ; motion comment\n",
        "(comment only); trailing comment\n",
        "M5\n",
    );

    assert_eq!(
        normalized_program_lines(source),
        ["G21", "G1 X10  Y20", "M5"]
    );
}

#[test]
fn program_character_count_uses_utf8_wire_bytes_and_newline() {
    assert_eq!(FluidNcWebSocketAdapter::program_line_wire_bytes("G1 X1"), 6);
    assert_eq!(
        FluidNcWebSocketAdapter::program_line_wire_bytes("G1 (µ)"),
        "G1 (µ)".len() + 1
    );
}

#[test]
fn reads_axis_count_from_fluidnc_status_fields() {
    assert_eq!(
        FluidNcWebSocketAdapter::reported_axis_count("<Idle|MPos:1,2,3,4,5,6|FS:0,0>"),
        Some(6)
    );
    assert_eq!(
        FluidNcWebSocketAdapter::reported_axis_count("<Idle|WPos:1,2,3|FS:0,0>"),
        Some(3)
    );
    assert_eq!(
        FluidNcWebSocketAdapter::reported_axis_count("<Idle|FS:0,0>"),
        None
    );
}
