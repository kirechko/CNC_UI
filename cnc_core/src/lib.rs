 //! Defines the shared CNC API types, status models, and controller interface.
use serde::{Deserialize, Serialize};
use std::path::Path;

pub mod adapters;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    X,
    Y,
    Z,
    A,
    B,
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
    /// Максимальный ход оси; `None`, если адаптер не смог прочитать значение.
    pub max_travel: Option<f64>,
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
    pub homed: [bool; 5],
    pub homing: bool,
}

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

pub type CncResult<T> = Result<T, ControllerError>;

#[async_trait::async_trait]
pub trait LinuxCncApi: Send + Sync {
    async fn status(&self) -> CncResult<MachineStatus>;

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
    async fn jog_continuous(
        &self,
        axis: Axis,
        direction: i8,
        feed_rate: f64,
    ) -> CncResult<()>;

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

    fn assert_core_adapter<T: LinuxCncApi>() {}

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
            serde_json::from_value(serde_json::json!({ "x": 1.0, "y": 2.0, "z": 3.0 }))
                .unwrap();

        assert_eq!(position.a, 0.0);
        assert_eq!(position.b, 0.0);
    }

    #[allow(dead_code)]
    fn assert_load_program_accepts_paths(_: &Path) {}
}
