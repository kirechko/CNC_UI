//! Core serializable CNC domain types: axes, machine state enums, status
//! snapshots, configuration models, controller errors, events and command
//! payloads. These live at the crate root (re-exported from `lib.rs`) so every
//! adapter and UI layer shares one vocabulary.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
    A,
    B,
    C,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskMode {
    Manual,
    Mdi,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Estop,
    EstopReset,
    Off,
    On,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterpreterState {
    Idle,
    Reading,
    Paused,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpindleDirection {
    Stopped,
    Clockwise,
    CounterClockwise,
}

/// Kind of motion for a toolpath segment; serialized as snake_case (`rapid`, `linear`,
/// `arc_clockwise`, `arc_counter_clockwise`) to match the web UI contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionKind {
    Rapid,
    Linear,
    ArcClockwise,
    ArcCounterClockwise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinateSystem {
    G54,
    G55,
    G56,
    G57,
    G58,
    G59,
    G59_1,
    G59_2,
    G59_3,
}

impl CoordinateSystem {
    pub const ALL: [Self; 9] = [
        Self::G54,
        Self::G55,
        Self::G56,
        Self::G57,
        Self::G58,
        Self::G59,
        Self::G59_1,
        Self::G59_2,
        Self::G59_3,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::G54 => "G54",
            Self::G55 => "G55",
            Self::G56 => "G56",
            Self::G57 => "G57",
            Self::G58 => "G58",
            Self::G59 => "G59",
            Self::G59_1 => "G59.1",
            Self::G59_2 => "G59.2",
            Self::G59_3 => "G59.3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CoordinateSystemOffset {
    pub system: CoordinateSystem,
    /// Work coordinate offset in machine coordinates.
    pub offset: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MachineAxis {
    X,
    Y,
    Z,
    A,
    B,
    C,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AxisUnits {
    Linear,
    Angular,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AxisConfiguration {
    pub axis: MachineAxis,
    pub units: AxisUnits,
    /// Maximum axis travel in millimeters or degrees.
    pub max_travel: Option<f64>,
    /// Maximum axis velocity in units/min.
    pub max_velocity: Option<f64>,
    /// Axis acceleration in units/s².
    pub acceleration: Option<f64>,
    /// Motor steps per axis unit.
    pub steps_per_unit: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpindleConfiguration {
    /// Minimum spindle speed in RPM.
    pub min_rpm: Option<f64>,
    /// Maximum spindle speed in RPM.
    pub max_rpm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineConfiguration {
    pub axes: Vec<AxisConfiguration>,
    pub spindle: SpindleConfiguration,
    /// Numeric controller settings, for example FluidNC parameters from `$$`.
    /// Keys are setting numbers without the `$` character.
    pub settings: HashMap<u16, f64>,
}

impl MachineConfiguration {
    pub fn axis_count(&self) -> usize {
        self.axes.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    #[serde(default)]
    pub a: f64,
    #[serde(default)]
    pub b: f64,
    #[serde(default)]
    pub c: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpindleStatus {
    /// Commanded spindle speed.
    pub commanded_rpm: f64,
    /// Actual spindle speed; the emulator may simply repeat commanded_rpm.
    pub actual_rpm: f64,
    pub direction: SpindleDirection,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineStatus {
    pub connected: bool,
    pub task_mode: TaskMode,
    pub task_state: TaskState,
    pub interpreter_state: InterpreterState,
    pub position: Position,
    /// G54 work-zero offset in machine coordinates.
    pub work_offset: Position,
    pub active_coordinate_system: CoordinateSystem,
    pub coordinate_systems: Vec<CoordinateSystemOffset>,
    pub feed_rate: f64,
    pub spindle: SpindleStatus,
    pub mist_coolant_on: bool,
    pub flood_coolant_on: bool,
    /// Current Jog or rapid G0 feed rate in units/min.
    #[serde(default)]
    pub jog_rate: f64,
    /// Current motion type, for example Jog, G0 or G1.
    #[serde(default)]
    pub motion_mode: Option<String>,
    pub max_spindle_rpm: f64,
    pub loaded_program: Option<String>,
    pub current_line: u32,
    pub total_lines: u32,
    pub message: Option<String>,
    pub homed: [bool; 6],
    pub homing: bool,
    /// Universal toolpath progress: program line, traversed path length and
    /// tool position. Missing in older payloads, so it defaults to unavailable.
    #[serde(default)]
    pub progress: crate::ToolpathProgress,
}

/// An error returned by a CNC controller or adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerError {
    pub code: String,
    pub message: String,
}

impl ControllerError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// Program input accepted by a controller adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramSource {
    /// A path interpreted according to the adapter's documented server/client semantics.
    Path(std::path::PathBuf),
    /// Inline G-code content. Adapters may reject this when the controller cannot receive it.
    Inline(String),
}

/// CNC override channel to modify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverrideKind {
    /// Feed-rate override, expressed as a percentage.
    Feed,
    /// Spindle-speed override, expressed as a percentage.
    Spindle,
    /// Rapid-traverse override, expressed as a percentage.
    Rapid,
}

/// Coarse UI machine state used in state transition events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    /// No active controller connection.
    Disconnected,
    /// Controller is idle and ready.
    Idle,
    /// Program execution or motion is running.
    Run,
    /// Motion is held or program execution is paused.
    Hold,
    /// Jog motion is active.
    Jog,
    /// Homing is active.
    Home,
    /// Controller reports an alarm condition.
    Alarm,
    /// Controller reports a door interlock condition.
    Door,
    /// Controller is in check/dry-run mode.
    Check,
    /// Controller is asleep.
    Sleep,
    /// The controller state is not recognized.
    Unknown,
}

/// Event emitted by a CNC adapter for UI consumers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CncEvent {
    /// Transport connection was established.
    Connected,
    /// Transport connection was lost.
    Disconnected { reason: String },
    /// A fresh status snapshot was published.
    StatusUpdate,
    /// The coarse UI state changed.
    StateChanged {
        from: MachineState,
        to: MachineState,
    },
    /// The acknowledged or reported program line changed.
    ProgramLineChanged { line: u32 },
    /// Program execution started.
    ProgramStarted,
    /// Program execution finished.
    ProgramFinished,
    /// Controller-reported alarm.
    Alarm { code: String, message: String },
    /// Adapter detected a protocol inconsistency.
    ProtocolError { code: String, message: String },
    /// Controller-reported override value changed.
    OverrideChanged { kind: OverrideKind, value: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct JogRequest {
    pub axis: Axis,
    /// Direction: -1 or +1.
    pub direction: i8,
    /// Distance in the units of the current program.
    pub distance: f64,
    pub feed_rate: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpindleCommand {
    pub direction: SpindleDirection,
    pub rpm: f64,
}

/// Result type shared by CNC adapters.
pub type CncResult<T> = Result<T, ControllerError>;
