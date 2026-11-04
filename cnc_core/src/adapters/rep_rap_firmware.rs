//! Experimental RepRapFirmware HTTP/JSON adapter for Duet-family controllers.
//!
//! This adapter uses the RRF v3 HTTP endpoints `/machine/status` and
//! `/machine/code`. It deliberately reports missing telemetry as unavailable,
//! and does not map HTTP G-code to a safety-rated emergency stop.

use crate::{
    Axis, CncApi, CncResult, ControllerError, CoordinateSystem, InterpreterState, JogRequest,
    MachineConfiguration, MachineStatus, Position, SpindleCommand, SpindleDirection, SpindleMode,
    SpindleStatus, TaskMode, TaskState, ToolpathProgress,
};
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio::sync::Mutex;

/// HTTP connection settings for one RepRapFirmware controller.
#[derive(Debug, Clone)]
pub struct RepRapFirmwareConfig {
    /// Base URL such as `http://duet.local`.
    pub base_url: String,
    /// Request timeout applied to status and command exchanges.
    pub request_timeout: Duration,
    /// Optional HTTP Basic password for protected Duet endpoints.
    pub password: Option<String>,
}

impl RepRapFirmwareConfig {
    /// Builds a controller configuration with a conservative request timeout.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            request_timeout: Duration::from_secs(5),
            password: None,
        }
    }
}

/// Adapter for RepRapFirmware's HTTP object-model and G-code endpoints.
pub struct RepRapFirmwareAdapter {
    /// HTTP endpoint and optional authentication settings.
    config: RepRapFirmwareConfig,
    /// Reusable HTTP connection pool configured with the request timeout.
    client: reqwest::Client,
    /// Host-side program and spindle metadata.
    session: Mutex<Session>,
}

/// Host-side metadata not exposed consistently across RRF versions.
struct Session {
    /// Loaded program lines used for optional UI bookkeeping.
    program: Vec<String>,
    /// File name of the loaded host-side program.
    program_name: Option<String>,
    /// Number of program lines successfully submitted.
    current_line: u32,
    /// Last spindle request sent by this adapter.
    spindle: SpindleCommand,
}

impl RepRapFirmwareAdapter {
    /// Creates an adapter without making network requests during construction.
    pub fn connect(config: RepRapFirmwareConfig) -> CncResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| ControllerError::new("connect", error.to_string()))?;
        Ok(Self {
            config,
            client,
            session: Mutex::new(Session {
                program: Vec::new(),
                program_name: None,
                current_line: 0,
                spindle: SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
            }),
        })
    }

    /// Reads the object model and unwraps the common `result` response envelope.
    async fn object_model(&self) -> CncResult<Value> {
        let mut request = self
            .client
            .get(format!("{}/machine/status", self.config.base_url));
        if let Some(password) = &self.config.password {
            request = request.basic_auth("", Some(password));
        }
        let response = request
            .send()
            .await
            .map_err(|error| ControllerError::new("transport", error.to_string()))?;
        if !response.status().is_success() {
            return Err(ControllerError::new(
                "http",
                format!("RepRapFirmware returned HTTP {}", response.status()),
            ));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|error| ControllerError::new("protocol", error.to_string()))?;
        Ok(body.get("result").cloned().unwrap_or(body))
    }

    /// Sends one G-code line via `/machine/code` and rejects non-success HTTP responses.
    async fn send_code(&self, code: &str) -> CncResult<()> {
        if code.trim().is_empty() || code.contains(['\r', '\n']) {
            return Err(ControllerError::new(
                "bad_command",
                "RepRapFirmware accepts exactly one non-empty command line",
            ));
        }
        let mut request = self
            .client
            .get(format!("{}/machine/code", self.config.base_url))
            .query(&[("gcode", code)]);
        if let Some(password) = &self.config.password {
            request = request.basic_auth("", Some(password));
        }
        let response = request
            .send()
            .await
            .map_err(|error| ControllerError::new("transport", error.to_string()))?;
        if !response.status().is_success() {
            return Err(ControllerError::new(
                "controller",
                format!("RepRapFirmware returned HTTP {}", response.status()),
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl CncApi for RepRapFirmwareAdapter {
    /// Queries RRF object-model status and maps only known fields.
    async fn status(&self) -> CncResult<MachineStatus> {
        let model = self.object_model().await?;
        let current = model.pointer("move.current").and_then(Value::as_array);
        let coord = |index: usize| {
            current
                .and_then(|values| values.get(index))
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
                .unwrap_or(0.0)
        };
        let firmware_state = model
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let state = firmware_state.to_ascii_lowercase();
        let busy = matches!(state.as_str(), "processing" | "busy");
        let halted = state == "halted";
        let session = self.session.lock().await;
        let actual_rpm = model
            .pointer("spindles.0.current")
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite())
            .unwrap_or(0.0);
        Ok(MachineStatus {
            connected: true,
            task_mode: if busy {
                TaskMode::Auto
            } else {
                TaskMode::Manual
            },
            task_state: if halted {
                TaskState::Estop
            } else {
                TaskState::On
            },
            interpreter_state: if busy {
                InterpreterState::Reading
            } else if halted {
                InterpreterState::Paused
            } else {
                InterpreterState::Idle
            },
            position: Position {
                x: coord(0),
                y: coord(1),
                z: coord(2),
                a: coord(3),
                b: coord(4),
                c: coord(5),
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
            feed_rate: model
                .pointer("move.speedFactor")
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
                .unwrap_or(0.0),
            spindle: SpindleStatus {
                commanded_rpm: session.spindle.rpm,
                actual_rpm,
                direction: if actual_rpm > 0.0 {
                    session.spindle.direction
                } else {
                    SpindleDirection::Stopped
                },
                enabled: actual_rpm > 0.0,
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
            current_tool: model
                .pointer("state.currentTool.number")
                .and_then(Value::as_u64)
                .map(|value| value as u16),
            loaded_program: session.program_name.clone(),
            current_line: session.current_line,
            total_lines: session.program.len() as u32,
            message: Some(format!(
                "RepRapFirmware: {firmware_state}; unsupported telemetry omitted"
            )),
            homed: [false; 6],
            homing: false,
            progress: ToolpathProgress::default(),
        })
    }
    /// Explicitly rejects configuration until model-version-specific limit mapping is verified.
    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        Err(unsupported("machine configuration"))
    }
    /// No generic host software-enable operation is assumed.
    async fn power_on(&self) -> CncResult<()> {
        Err(unsupported("software power-on"))
    }
    /// Requires configured hardware output control.
    async fn power_off(&self) -> CncResult<()> {
        Err(unsupported("software power-off"))
    }
    /// HTTP G-code does not provide a safety-rated emergency stop.
    async fn estop(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "use the Duet hardware E-Stop; HTTP G-code is not an emergency stop",
        ))
    }
    /// Requests homing for all axes or a supported linear axis.
    async fn home(&self, axis: Option<Axis>) -> CncResult<()> {
        self.send_code(match axis {
            None => "G28",
            Some(Axis::X) => "G28 X",
            Some(Axis::Y) => "G28 Y",
            Some(Axis::Z) => "G28 Z",
            _ => return Err(unsupported("rotary-axis homing")),
        })
        .await
    }
    /// Selects a conventional work coordinate system.
    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        self.send_code(system.label()).await
    }
    /// Zeros XYZ for a selected work coordinate system.
    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        self.send_code(&format!("G10 L20 P{} X0 Y0 Z0", coordinate_number(system)))
            .await
    }
    /// Zeros one axis of the default work coordinate system.
    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        self.send_code(&format!("G10 L20 P1 {}0", axis_letter(axis)))
            .await
    }
    /// Validates and sends one bounded incremental movement.
    async fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        if ![-1, 1].contains(&request.direction)
            || !request.distance.is_finite()
            || request.distance <= 0.0
        {
            return Err(ControllerError::new("bad_jog", "invalid jog request"));
        }
        let feed = request.feed_rate.unwrap_or(100.0);
        if !feed.is_finite() || feed <= 0.0 {
            return Err(ControllerError::new("bad_jog", "jog feed must be positive"));
        }
        self.send_code(&format!(
            "G91 G1 {}{:.4} F{feed:.1} G90",
            axis_letter(request.axis),
            request.distance * f64::from(request.direction)
        ))
        .await
    }
    /// Refuses continuous jog until its stop semantics are verified on supported RRF versions.
    async fn jog_continuous(&self, _axis: Axis, _direction: i8, _feed_rate: f64) -> CncResult<()> {
        Err(unsupported("continuous jog"))
    }
    /// Refuses coordinated continuous jog pending firmware and machine safety validation.
    async fn jog_continuous_xy(
        &self,
        _x_direction: i8,
        _y_direction: i8,
        _feed_rate: f64,
    ) -> CncResult<()> {
        Err(unsupported("continuous XY jog"))
    }
    /// Refuses to guess a realtime cancellation byte.
    async fn cancel_jog(&self) -> CncResult<()> {
        Err(unsupported("jog cancellation"))
    }
    /// Sends one G-code command.
    async fn mdi(&self, command: &str) -> CncResult<()> {
        self.send_code(command).await
    }
    /// Loads and prepares a host-side program for line-by-line command submission.
    async fn load_program(&self, path: &Path) -> CncResult<()> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        let lines = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with(';') && !line.starts_with('('))
            .map(str::to_owned)
            .collect();
        let mut session = self.session.lock().await;
        session.program = lines;
        session.program_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        session.current_line = 0;
        Ok(())
    }
    /// Sends program lines sequentially and updates host-side acknowledgement bookkeeping.
    async fn program_run(&self, start_line: u32) -> CncResult<()> {
        let mut session = self.session.lock().await;
        let start = start_line as usize;
        if start >= session.program.len() {
            return Err(ControllerError::new(
                "bad_program",
                "start line is outside loaded program",
            ));
        }
        for index in start..session.program.len() {
            self.send_code(&session.program[index]).await?;
            session.current_line = index as u32 + 1;
        }
        Ok(())
    }
    /// Requests controlled pause with the RRF M25 command.
    async fn program_pause(&self) -> CncResult<()> {
        self.send_code("M25").await
    }
    /// Resume command differs by firmware and job-source mode; no guess is sent.
    async fn program_resume(&self) -> CncResult<()> {
        Err(unsupported("program resume"))
    }
    /// Requests controlled program stop with M0; this is not an emergency stop.
    async fn program_abort(&self) -> CncResult<()> {
        self.send_code("M0").await
    }
    /// Controls spindle direction and RPM with standard M-codes.
    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        if !command.rpm.is_finite() || command.rpm < 0.0 {
            return Err(ControllerError::new(
                "bad_spindle",
                "spindle RPM must be finite and non-negative",
            ));
        }
        let code = match command.direction {
            SpindleDirection::Stopped => "M5".into(),
            SpindleDirection::Clockwise => format!("M3 S{:.0}", command.rpm),
            SpindleDirection::CounterClockwise => format!("M4 S{:.0}", command.rpm),
        };
        self.send_code(&code).await?;
        self.session.lock().await.spindle = command;
        Ok(())
    }
    /// Refuses an unverified alarm-clear sequence.
    async fn clear_error(&self) -> CncResult<()> {
        Err(unsupported("alarm clearing"))
    }
}

/// Produces a clear unsupported-capability error for RRF.
fn unsupported(feature: &str) -> ControllerError {
    ControllerError::new(
        "unsupported",
        format!(
            "RepRapFirmware {feature} is unavailable until version-specific behavior is validated"
        ),
    )
}
/// Maps standard coordinate-system enums to G10 L20 P values.
fn coordinate_number(system: CoordinateSystem) -> u8 {
    match system {
        CoordinateSystem::G54 => 1,
        CoordinateSystem::G55 => 2,
        CoordinateSystem::G56 => 3,
        CoordinateSystem::G57 => 4,
        CoordinateSystem::G58 => 5,
        CoordinateSystem::G59 => 6,
        CoordinateSystem::G59_1 => 7,
        CoordinateSystem::G59_2 => 8,
        CoordinateSystem::G59_3 => 9,
    }
}
/// Maps axis enums to standard G-code letters.
fn axis_letter(axis: Axis) -> char {
    match axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
        Axis::A => 'A',
        Axis::B => 'B',
        Axis::C => 'C',
    }
}
