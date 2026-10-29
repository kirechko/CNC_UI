//! Status snapshot type, spindle-direction UI enum and the alert helpers
//! shared by the controller facade and the worker thread.

use crate::cnc_api::MachineStatus;
use std::sync::{Arc, Mutex};

/// Everything the UI needs to know about the machine at one glance.
///
/// The struct is cloned out of the mutex for each `/api/status` poll, so it
/// must stay cheap and self-contained.
#[derive(Debug, Clone)]
pub struct ControllerSnapshot {
    /// Latest machine status reported by the adapter.
    pub machine: MachineStatus,
    /// Axis/spindle configuration, fetched after (re)connection.
    pub machine_configuration: Option<crate::cnc_api::MachineConfiguration>,
    /// Current incremental jog step in mm (or degrees for rotary axes).
    pub jog_step: f64,
    /// Worker error shown in the UI as a (kind, text) pair.
    pub ui_alert: Option<(String, String)>,
    /// Parsed toolpath of the loaded program; empty when no program is loaded.
    pub toolpath: cnc_core::Toolpath,
}

/// UI-level spindle direction; the adapter works with raw S-values.
#[derive(Debug, Clone, PartialEq)]
pub enum SpindleDirection {
    Off,
    CW,
    Ccw,
}

/// Poll cadence for the worker thread: FluidNC's WebSocket should not be
/// flooded with realtime `?` queries faster than this.
pub(super) const STATUS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Maps a controller error code to the (kind, message) alert pair shown in the UI.
pub(super) fn error_alert(code: &str, message: &str) -> (String, String) {
    let kind = match code {
        "soft_limit" => "axis_limit",
        "connect" | "connection" | "io" | "socket" | "disconnected" => "connection",
        _ => "command_rejected",
    };
    (kind.to_string(), message.to_string())
}

/// Stores an alert into the shared snapshot (worker-side convenience).
pub(super) fn set_ui_alert(
    shared_status: &Arc<Mutex<ControllerSnapshot>>,
    code: &str,
    message: &str,
) {
    shared_status.lock().unwrap().ui_alert = Some(error_alert(code, message));
}

/// Records an adapter message into the journal, suppressing exact repeats.
///
/// `last_message` keeps the previously written severity+text key; FluidNC can
/// emit the same alarm line on every status poll, and the journal should only
/// contain the first occurrence.
pub(super) fn record_adapter_message(
    last_message: &mut Option<String>,
    severity: &str,
    message: &str,
) {
    if message.trim().is_empty() {
        *last_message = None;
        return;
    }

    let key = format!("{severity}\u{1f}{message}");
    if last_message.as_deref() != Some(key.as_str()) {
        crate::message_log::record(severity, "CNC/adapter", message);
        *last_message = Some(key);
    }
}

/// Builds the disconnected status snapshot shown before the first controller response.
pub(super) fn initial_disconnected_status() -> ControllerSnapshot {
    ControllerSnapshot {
        machine: MachineStatus {
            connected: false,
            task_mode: crate::cnc_api::TaskMode::Manual,
            task_state: crate::cnc_api::TaskState::Off,
            interpreter_state: crate::cnc_api::InterpreterState::Idle,
            position: crate::cnc_api::Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                a: 0.0,
                b: 0.0,
                c: 0.0,
            },
            work_offset: crate::cnc_api::Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                a: 0.0,
                b: 0.0,
                c: 0.0,
            },
            active_coordinate_system: crate::cnc_api::CoordinateSystem::G54,
            coordinate_systems: Vec::new(),
            feed_rate: 0.0,
            spindle: crate::cnc_api::SpindleStatus {
                commanded_rpm: 0.0,
                actual_rpm: 0.0,
                direction: crate::cnc_api::SpindleDirection::Stopped,
                enabled: false,
            },
            mist_coolant_on: false,
            flood_coolant_on: false,
            jog_rate: 0.0,
            motion_mode: None,
            max_spindle_rpm: 0.0,
            loaded_program: None,
            current_line: 0,
            total_lines: 0,
            message: Some("Waiting for CNC emulator connection".into()),
            homed: [false; 6],
            homing: false,
            progress: cnc_core::ToolpathProgress::default(),
        },
        machine_configuration: None,
        jog_step: 1.0,
        ui_alert: Some((
            "connection".into(),
            "Waiting for CNC emulator connection".into(),
        )),
        toolpath: cnc_core::Toolpath::default(),
    }
}
