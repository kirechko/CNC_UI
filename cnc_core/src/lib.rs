//! Defines the shared CNC API types, status models, discovery, and controller interface.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub mod adapters;
pub mod discovery;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Смещение рабочего нуля в машинных координатах.
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
    /// Максимальный ход оси в миллиметрах или градусах.
    pub max_travel: Option<f64>,
    /// Максимальная скорость оси в единицах/мин.
    pub max_velocity: Option<f64>,
    /// Ускорение оси в единицах/с².
    pub acceleration: Option<f64>,
    /// Шаги двигателя на единицу оси.
    pub steps_per_unit: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpindleConfiguration {
    /// Минимальная частота вращения, RPM.
    pub min_rpm: Option<f64>,
    /// Максимальная частота вращения, RPM.
    pub max_rpm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineConfiguration {
    pub axes: Vec<AxisConfiguration>,
    pub spindle: SpindleConfiguration,
    /// Числовые настройки контроллера, например параметры FluidNC из `$$`.
    /// Ключи — номера настроек без символа `$`.
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
    /// Запрошенные обороты.
    pub commanded_rpm: f64,
    /// Фактические обороты; эмулятор может повторять commanded_rpm.
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
    /// Текущая скорость Jog или быстрого перемещения G0, в единицах/мин.
    #[serde(default)]
    pub jog_rate: f64,
    /// Тип текущего движения, например Jog, G0 или G1.
    #[serde(default)]
    pub motion_mode: Option<String>,
    pub max_spindle_rpm: f64,
    pub loaded_program: Option<String>,
    pub current_line: u32,
    pub total_lines: u32,
    pub message: Option<String>,
    pub homed: [bool; 6],
    pub homing: bool,
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
    /// Направление: -1 или +1.
    pub direction: i8,
    /// Перемещение в единицах текущей программы.
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

/// Return the coarse UI state represented by a controller status snapshot.
pub fn machine_state(status: &MachineStatus) -> MachineState {
    if !status.connected {
        MachineState::Disconnected
    } else if status.task_state == TaskState::Estop
        || status
            .message
            .as_deref()
            .is_some_and(|m| m.starts_with("ALARM:"))
    {
        MachineState::Alarm
    } else if status.homing {
        MachineState::Home
    } else if status
        .motion_mode
        .as_deref()
        .is_some_and(|mode| mode.eq_ignore_ascii_case("jog"))
    {
        MachineState::Jog
    } else if status.interpreter_state == InterpreterState::Paused {
        MachineState::Hold
    } else if status.interpreter_state == InterpreterState::Reading {
        MachineState::Run
    } else if status.task_state == TaskState::On {
        MachineState::Idle
    } else {
        MachineState::Unknown
    }
}

/// Shared asynchronous control API implemented by CNC backends.
#[async_trait::async_trait]
pub trait CncApi: Send + Sync {
    /// Query the controller and return its most recent complete status.
    async fn status(&self) -> CncResult<MachineStatus>;

    /// Return endpoints found by automatic discovery, if this adapter supports it.
    fn discovered_devices(&self) -> Vec<discovery::DiscoveredDevice> {
        Vec::new()
    }

    /// Start a discovery pass and stream each result as it is found.
    async fn rediscover_devices_stream(
        &self,
        _priority_targets: Vec<String>,
    ) -> CncResult<tokio::sync::mpsc::Receiver<discovery::DiscoveredDevice>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        for device in self.discovered_devices() {
            if sender.send(device).await.is_err() {
                break;
            }
        }
        Ok(receiver)
    }

    /// Run a new device discovery pass, if supported by this adapter.
    async fn rediscover_devices(&self) -> CncResult<Vec<discovery::DiscoveredDevice>> {
        let mut receiver = self.rediscover_devices_stream(Vec::new()).await?;
        let mut devices = Vec::new();
        while let Some(device) = receiver.recv().await {
            devices.push(device);
        }
        Ok(devices)
    }

    /// Stable identifier of the endpoint currently used by this adapter, when known.
    fn connected_device_id(&self) -> Option<String> {
        None
    }

    /// Return a nonblocking copy of the most recently published status, when supported.
    fn last_status(&self) -> Option<MachineStatus> {
        None
    }

    /// Subscribe to status snapshot changes, when supported by the adapter.
    fn subscribe_status(&self) -> Option<tokio::sync::watch::Receiver<MachineStatus>> {
        None
    }

    /// Subscribe to adapter events, when supported by the adapter.
    fn subscribe_events(&self) -> Option<tokio::sync::broadcast::Receiver<CncEvent>> {
        None
    }

    /// Load a path or inline program. The default supports existing path-based adapters only.
    async fn load_program_source(&self, source: ProgramSource) -> CncResult<()> {
        match source {
            ProgramSource::Path(path) => self.load_program(&path).await,
            ProgramSource::Inline(_) => Err(ControllerError::new(
                "unsupported",
                "inline program loading is not supported by this adapter",
            )),
        }
    }

    /// Set a feed, spindle, or rapid override as a percentage.
    async fn set_override(&self, kind: OverrideKind, value: f64) -> CncResult<()> {
        let _ = (kind, value);
        Err(ControllerError::new(
            "unsupported",
            "overrides are not supported by this adapter",
        ))
    }

    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        Err(ControllerError::new(
            "unsupported",
            "machine configuration is not available for this adapter",
        ))
    }

    async fn power_on(&self) -> CncResult<()>;
    async fn power_off(&self) -> CncResult<()>;
    async fn estop(&self) -> CncResult<()>;
    async fn home(&self, axis: Option<Axis>) -> CncResult<()>;
    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()>;
    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()>;
    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()>;
    async fn jog_increment(&self, request: JogRequest) -> CncResult<()>;
    async fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()>;

    async fn jog_continuous_xy(
        &self,
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        let _ = (x_direction, y_direction, feed_rate);
        Err(ControllerError::new(
            "unsupported",
            "coordinated continuous XY jog is not supported by this adapter",
        ))
    }

    async fn cancel_jog(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "jog cancellation is not supported by this adapter",
        ))
    }

    /// Остановить текущее управляемое движение, не выполняя аварийный останов.
    async fn stop_motion(&self) -> CncResult<()> {
        self.cancel_jog().await
    }

    /// Выполнить команду контроллера и вернуть текстовый ответ, если он доступен.
    /// Может поддерживать G-code и команды, специфичные для адаптера.
    async fn execute_command(
        &self,
        command: &str,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> CncResult<String> {
        if cancellation.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ControllerError::new(
                "command_cancelled",
                "Команда отменена.",
            ));
        }
        self.mdi(command).await?;
        if cancellation.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ControllerError::new(
                "command_cancelled",
                "Команда отменена.",
            ));
        }
        Ok("Команда принята контроллером.".into())
    }

    async fn mdi(&self, command: &str) -> CncResult<()>;
    async fn load_program(&self, path: &Path) -> CncResult<()>;
    async fn program_run(&self, start_line: u32) -> CncResult<()>;
    async fn program_pause(&self) -> CncResult<()>;
    async fn program_resume(&self) -> CncResult<()>;
    async fn program_abort(&self) -> CncResult<()>;
    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()>;
    async fn clear_error(&self) -> CncResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn assert_core_adapter<T: CncApi>() {}

    #[test]
    fn migrated_adapters_implement_the_core_api() {
        assert_core_adapter::<adapters::debug::LinuxCncAdapter>();
        assert_core_adapter::<adapters::debug::FluidNcAdapter>();
        assert_core_adapter::<adapters::emulator::EmAdapter>();
        assert_core_adapter::<adapters::grbl::GrblAdapter>();
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
}
