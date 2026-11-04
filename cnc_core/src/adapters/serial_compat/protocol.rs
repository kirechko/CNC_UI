//! Serial framing and firmware-specific status decoding for Marlin and TinyG.

use super::{SerialDialect, Session};
use crate::{CncResult, ControllerError, InterpreterState, MachineStatus};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

/// Sends one firmware command using its native request framing and waits for a response.
pub(super) async fn exchange(session: &mut Session, command: &str) -> CncResult<Vec<String>> {
    if command.contains(['\r', '\n']) {
        return Err(ControllerError::new(
            "bad_command",
            "serial commands must contain one line",
        ));
    }
    let line = match session.dialect {
        SerialDialect::Marlin => format!("{command}\n"),
        SerialDialect::TinyG if command.trim_start().starts_with('{') => {
            let value: serde_json::Value = serde_json::from_str(command)
                .map_err(|error| ControllerError::new("bad_command", error.to_string()))?;
            format!("{}\n", value)
        }
        SerialDialect::TinyG => {
            let escaped = serde_json::to_string(command)
                .map_err(|error| ControllerError::new("bad_command", error.to_string()))?;
            format!("{{\"gc\":{escaped}}}\n")
        }
    };
    session
        .port
        .write_all(line.as_bytes())
        .await
        .map_err(|error| ControllerError::new("io", error.to_string()))?;
    let mut responses = Vec::new();
    loop {
        let response = read_line(session).await?;
        if is_ack(session.dialect, &response) {
            if session.dialect == SerialDialect::TinyG {
                responses.push(response);
            }
            return Ok(responses);
        }
        if is_error(session.dialect, &response) {
            return Err(ControllerError::new("controller", response));
        }
        responses.push(response);
    }
}

/// Reads one bounded newline-delimited response from the serial port.
async fn read_line(session: &mut Session) -> CncResult<String> {
    loop {
        if let Some(index) = session
            .receive_buffer
            .iter()
            .position(|byte| *byte == b'\n')
        {
            if index > 8192 {
                return Err(ControllerError::new(
                    "protocol",
                    "serial response line exceeded limit",
                ));
            }
            let bytes: Vec<_> = session.receive_buffer.drain(..=index).collect();
            return String::from_utf8(bytes[..bytes.len() - 1].to_vec())
                .map(|line| line.trim().to_owned())
                .map_err(|error| ControllerError::new("protocol", error.to_string()));
        }
        if session.receive_buffer.len() > 8192 {
            return Err(ControllerError::new(
                "protocol",
                "serial response line exceeded limit",
            ));
        }
        let mut chunk = [0u8; 256];
        let count = timeout(
            super::SerialCompatAdapter::response_timeout(),
            session.port.read(&mut chunk),
        )
        .await
        .map_err(|_| ControllerError::new("timeout", "timed out waiting for serial response"))?
        .map_err(|error| ControllerError::new("io", error.to_string()))?;
        if count == 0 {
            return Err(ControllerError::new(
                "disconnected",
                "serial controller disconnected",
            ));
        }
        session.receive_buffer.extend_from_slice(&chunk[..count]);
    }
}

/// Determines whether a line is the native positive acknowledgement for a dialect.
fn is_ack(dialect: SerialDialect, line: &str) -> bool {
    match dialect {
        SerialDialect::Marlin => line == "ok",
        SerialDialect::TinyG => serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| value.get("r").cloned())
            .is_some(),
    }
}

/// Detects native controller errors without treating arbitrary status messages as success.
fn is_error(dialect: SerialDialect, line: &str) -> bool {
    match dialect {
        SerialDialect::Marlin => line.starts_with("Error:") || line.starts_with("!!"),
        SerialDialect::TinyG => serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| {
                value
                    .get("er")
                    .and_then(|error| error.get("status"))
                    .and_then(serde_json::Value::as_i64)
            })
            .is_some_and(|status| status != 0),
    }
}

/// Parses the latest firmware status into the shared CNC status model.
pub(super) fn parse_status(dialect: SerialDialect, lines: &[String]) -> CncResult<MachineStatus> {
    match dialect {
        SerialDialect::Marlin => parse_marlin_status(lines),
        SerialDialect::TinyG => parse_tinyg_status(lines),
    }
}

/// Parses a Marlin `M114` response with an explicit finite-number check for every reported axis.
fn parse_marlin_status(lines: &[String]) -> CncResult<MachineStatus> {
    let line = lines
        .iter()
        .rev()
        .find(|line| line.contains(" X:") || line.starts_with("X:"))
        .ok_or_else(|| {
            ControllerError::new("protocol", "Marlin M114 response contains no position")
        })?;
    let mut coordinates = [0.0; 6];
    for (index, axis) in ['X', 'Y', 'Z', 'A', 'B', 'C'].iter().enumerate() {
        if let Some(value) = find_axis_value(line, *axis) {
            coordinates[index] = value
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    ControllerError::new("protocol", format!("invalid Marlin {axis} position"))
                })?;
        }
    }
    Ok(status_from_coordinates(
        coordinates,
        InterpreterState::Idle,
        "Marlin",
    ))
}

/// Parses TinyG's JSON status report and validates position fields before exposing them.
fn parse_tinyg_status(lines: &[String]) -> CncResult<MachineStatus> {
    for line in lines.iter().rev() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(machine) = value.get("sr") else {
            continue;
        };
        let mut coordinates = [0.0; 6];
        for (index, axis) in ["posx", "posy", "posz", "posa", "posb", "posc"]
            .iter()
            .enumerate()
        {
            let field = machine
                .get(axis)
                .or_else(|| machine.get(axis.strip_prefix("pos").unwrap_or(axis)));
            if let Some(value) = field.and_then(serde_json::Value::as_f64) {
                if !value.is_finite() {
                    return Err(ControllerError::new(
                        "protocol",
                        "TinyG reported non-finite position",
                    ));
                }
                coordinates[index] = value;
            }
        }
        return Ok(status_from_coordinates(
            coordinates,
            InterpreterState::Idle,
            "TinyG",
        ));
    }
    Err(ControllerError::new(
        "protocol",
        "TinyG status response contains no status report",
    ))
}

/// Finds the numeric value following one axis marker in a Marlin position line.
fn find_axis_value<'a>(line: &'a str, axis: char) -> Option<&'a str> {
    let marker = format!("{axis}:");
    line.split_whitespace()
        .find_map(|field| field.strip_prefix(&marker))
}

/// Builds the conservative common status representation used by the serial compatibility adapters.
fn status_from_coordinates(
    position: [f64; 6],
    interpreter: InterpreterState,
    label: &str,
) -> MachineStatus {
    use crate::{
        CoordinateSystem, Position, SpindleDirection, SpindleMode, SpindleStatus, TaskMode,
        TaskState,
    };
    MachineStatus {
        connected: true,
        task_mode: TaskMode::Manual,
        task_state: TaskState::On,
        interpreter_state: interpreter,
        position: Position {
            x: position[0],
            y: position[1],
            z: position[2],
            a: position[3],
            b: position[4],
            c: position[5],
        },
        work_offset: Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            a: 0.0,
            b: 0.0,
            c: 0.0,
        },
        active_coordinate_system: CoordinateSystem::G54,
        coordinate_systems: Vec::new(),
        feed_rate: 0.0,
        spindle: SpindleStatus {
            commanded_rpm: 0.0,
            actual_rpm: 0.0,
            direction: SpindleDirection::Stopped,
            enabled: false,
            mode: SpindleMode::Standard,
            laser_power: None,
        },
        mist_coolant_on: false,
        flood_coolant_on: false,
        jog_rate: 0.0,
        motion_mode: None,
        max_spindle_rpm: 0.0,
        probe_triggered: false,
        probe_position: None,
        selected_tool: None,
        current_tool: None,
        loaded_program: None,
        current_line: 0,
        total_lines: 0,
        message: Some(format!(
            "{label} serial status; unsupported telemetry omitted"
        )),
        homed: [false; 6],
        homing: false,
        progress: crate::ToolpathProgress::default(),
    }
}
