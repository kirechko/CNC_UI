//! JSON data types exchanged with the browser UI and the Tauri commands.
//!
//! The field names of these structs are a stability contract with the frontend
//! (see the `contract_tests` in the parent module); renaming them requires
//! updating `web/js` in the same commit.

use crate::cnc_api::{Axis, InterpreterState, MachineConfiguration, TaskState};
use crate::cnc_controller::ControllerSnapshot;
use serde::{Deserialize, Serialize};

/// `?lang=` query parameter accepted by every handler that can return
/// user-facing error text.
#[derive(Deserialize, Default)]
pub(crate) struct LangQuery {
    pub(crate) lang: Option<String>,
}

/// Builds the response language from a parsed `lang` query and the raw
/// `Accept-Language` header.
pub(crate) fn request_language(query: &LangQuery, headers: &axum::http::HeaderMap) -> &'static str {
    let accept_language = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok());
    crate::i18n::negotiate(query.lang.as_deref(), accept_language)
}

/// One stored message-log record.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct LogEntry {
    pub(crate) id: i64,
    pub(crate) timestamp_ms: i64,
    pub(crate) severity: String,
    pub(crate) source: String,
    pub(crate) message: String,
}

/// The newest alarm record plus the newest non-alarm record, shown in the
/// header status area.
#[derive(Serialize)]
pub(crate) struct LatestLogMessages {
    pub(crate) critical: Option<LogEntry>,
    pub(crate) other: Option<LogEntry>,
}

/// Filter/sort/pagination parameters of `GET /api/log`.
#[derive(Deserialize)]
pub(crate) struct LogQuery {
    pub(crate) severity: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) search: Option<String>,
    pub(crate) sort: Option<String>,
    pub(crate) descending: Option<bool>,
    pub(crate) limit: Option<usize>,
    pub(crate) offset: Option<usize>,
    pub(crate) time_from_ms: Option<i64>,
    pub(crate) time_to_ms: Option<i64>,
}

/// One page of message-log records.
#[derive(Serialize)]
pub(crate) struct LogPage {
    pub(crate) entries: Vec<LogEntry>,
    pub(crate) total: i64,
}

/// Body of `POST /api/machines/select`: the discovery id chosen by the UI.
#[derive(Deserialize)]
pub(crate) struct MachineSelectRequest {
    pub(crate) id: String,
}

/// Whole machine list served to the machines dialog.
#[derive(Serialize)]
pub(crate) struct DiscoveredMachinesResponse {
    pub(crate) machines: Vec<DiscoveredMachine>,
    pub(crate) connected_address: Option<String>,
}

/// One discovered machine rendered in the machines dialog.
#[derive(Serialize)]
pub(crate) struct DiscoveredMachine {
    pub(crate) id: String,
    pub(crate) protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) machine_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) version: Option<String>,
    pub(crate) transport: String,
    pub(crate) address: String,
    pub(crate) connected: bool,
}

/// Program currently loaded into the backend (name + full text).
#[derive(Clone, Serialize)]
pub(crate) struct LoadedProgram {
    pub(crate) name: String,
    pub(crate) content: String,
}

/// Parses a coordinate system key from the API.
/// Unknown keys produce an error message translated into `language`.
pub(crate) fn parse_coordinate_system(
    value: &str,
    language: &str,
) -> Result<crate::cnc_api::CoordinateSystem, String> {
    use crate::cnc_api::CoordinateSystem::*;

    match value {
        "g54" | "G54" => Ok(G54),
        "g55" | "G55" => Ok(G55),
        "g56" | "G56" => Ok(G56),
        "g57" | "G57" => Ok(G57),
        "g58" | "G58" => Ok(G58),
        "g59" | "G59" => Ok(G59),
        "g59_1" | "G59.1" => Ok(G59_1),
        "g59_2" | "G59.2" => Ok(G59_2),
        "g59_3" | "G59.3" => Ok(G59_3),
        _ => Err(crate::i18n::translate(
            language,
            "api.error.unknownCoordinateSystem",
            &[("value", value.to_string())],
        )),
    }
}

pub(crate) fn coordinate_system_number(system: crate::cnc_api::CoordinateSystem) -> u8 {
    use crate::cnc_api::CoordinateSystem::*;

    match system {
        G54 => 1,
        G55 => 2,
        G56 => 3,
        G57 => 4,
        G58 => 5,
        G59 => 6,
        G59_1 => 7,
        G59_2 => 8,
        G59_3 => 9,
    }
}

pub(crate) fn axis_letter(axis: Axis) -> char {
    match axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
        Axis::A => 'A',
        Axis::B => 'B',
        Axis::C => 'C',
    }
}

/// Flat status payload consumed by `web/js/main.js` (`refreshStatus`).
#[derive(Serialize)]
pub(crate) struct StatusResponse {
    pub(crate) connected: bool,
    /// Machine state string; the `get_status` handler inspects it to decide
    /// whether the HTTP status trace should fire (READING/RUNNING states).
    pub(crate) state: String,
    x: f64,
    y: f64,
    z: f64,
    a: f64,
    b: f64,
    c: f64,
    machine_x: f64,
    machine_y: f64,
    machine_z: f64,
    machine_a: f64,
    machine_b: f64,
    machine_c: f64,
    coordinate_system: String,
    configuration: Option<MachineConfiguration>,
    movement_speed: f64,
    movement_mode: Option<String>,
    spindle_rpm: f64,
    spindle_commanded_rpm: f64,
    spindle_enabled: bool,
    spindle_direction: String,
    spindle_min_rpm: f64,
    spindle_max_rpm: f64,
    current_line: u32,
    total_lines: u32,
    message: Option<String>,
    error: Option<String>,
    /// Universal toolpath progress reported by the adapter (program line, traversed length, tool position).
    progress: cnc_core::ToolpathProgress,
    pub(crate) latest_critical: Option<LogEntry>,
    pub(crate) latest_other: Option<LogEntry>,
}

impl From<ControllerSnapshot> for StatusResponse {
    fn from(status: ControllerSnapshot) -> Self {
        let machine = status.machine;
        let spindle_min_rpm = status
            .machine_configuration
            .as_ref()
            .and_then(|configuration| configuration.spindle.min_rpm)
            .unwrap_or(0.0);
        let spindle_max_rpm = status
            .machine_configuration
            .as_ref()
            .and_then(|configuration| configuration.spindle.max_rpm)
            .unwrap_or(machine.max_spindle_rpm);

        let state = status_state_of(&machine);

        Self {
            connected: machine.connected,
            state: state.into(),
            x: machine.position.x - machine.work_offset.x,
            y: machine.position.y - machine.work_offset.y,
            z: machine.position.z - machine.work_offset.z,
            a: machine.position.a - machine.work_offset.a,
            b: machine.position.b - machine.work_offset.b,
            c: machine.position.c - machine.work_offset.c,
            machine_x: machine.position.x,
            machine_y: machine.position.y,
            machine_z: machine.position.z,
            machine_a: machine.position.a,
            machine_b: machine.position.b,
            machine_c: machine.position.c,
            coordinate_system: machine
                .active_coordinate_system
                .label()
                .to_ascii_lowercase()
                .replace('.', "_"),
            configuration: status.machine_configuration,
            movement_speed: if machine.jog_rate > 0.0 {
                machine.jog_rate
            } else {
                machine.feed_rate
            },
            movement_mode: machine.motion_mode,
            spindle_rpm: machine.spindle.actual_rpm,
            spindle_commanded_rpm: machine.spindle.commanded_rpm,
            spindle_enabled: machine.spindle.enabled,
            spindle_direction: match machine.spindle.direction {
                crate::cnc_api::SpindleDirection::Stopped => "stopped",
                crate::cnc_api::SpindleDirection::Clockwise => "cw",
                crate::cnc_api::SpindleDirection::CounterClockwise => "ccw",
            }
            .into(),
            spindle_min_rpm,
            spindle_max_rpm,
            current_line: machine.current_line,
            total_lines: machine.total_lines,
            message: machine.message,
            progress: machine.progress,
            error: status
                .ui_alert
                .map(|(kind, message)| format!("{kind}: {message}")),
            latest_critical: None,
            latest_other: None,
        }
    }
}

/// Maps a machine status to the UI state string (`IDLE`, `RUNNING`, ...).
fn status_state_of(machine: &crate::cnc_api::MachineStatus) -> &'static str {
    if machine.task_state == TaskState::Estop {
        "ALARM"
    } else if machine.homing {
        "HOMING"
    } else if !machine.connected {
        "DISCONNECTED"
    } else {
        match machine.interpreter_state {
            InterpreterState::Idle => "IDLE",
            InterpreterState::Reading => "READING",
            InterpreterState::Paused => "PAUSED",
            InterpreterState::Waiting => "WAITING",
        }
    }
}

/// Same mapping used by the log writer to detect status transitions.
pub(crate) fn status_state(status: &ControllerSnapshot) -> &'static str {
    status_state_of(&status.machine)
}
