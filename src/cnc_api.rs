 //! Defines the shared CNC API types, status models, and controller interface.
use serde::{Deserialize, Serialize};
use std::path::Path;

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

/// Общий интерфейс управления для UI.
///
/// Реализация может быть клиентом отдельного эмулятора или адаптером
/// к настоящему LinuxCNC. Методы отправляют команды; состояние UI получает
/// через `status()`.
pub trait LinuxCncApi: Send + Sync {
    /// Последний снимок состояния. Клиент обновляет его в фоне или по запросу.
    fn status(&self) -> CncResult<MachineStatus>;

    /// Конфигурация станка, если адаптер может получить её из контроллера.
    fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        Err(ControllerError::new(
            "unsupported",
            "machine configuration is not available for this adapter",
        ))
    }

    /// Сброс аварийного останова и включение станка.
    fn power_on(&self) -> CncResult<()>;

    /// Выключение станка.
    fn power_off(&self) -> CncResult<()>;

    /// Аварийный останов.
    fn estop(&self) -> CncResult<()>;
    fn home(&self, axis: Option<Axis>) -> CncResult<()>;
    /// Выбрать активную систему координат.
    fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()>;

    /// Установить ноль выбранной системы в текущей машинной позиции (G10 L2 Pn).
    fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()>;

    /// Установить ноль одной оси активной системы координат.
    fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()>;

    /// Инкрементальный Jog по оси.
    fn jog_increment(&self, request: JogRequest) -> CncResult<()>;

    /// Непрерывный Jog; direction=0 завершает движение.
    fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()>;

    /// Согласованный непрерывный Jog по XY с результирующей скоростью feed_rate.
    /// Нулевые направления означают остановку вектора.
    fn jog_continuous_xy(
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

    /// Немедленно отменить активное движение, запущенное командами Jog.
    fn cancel_jog(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "jog cancellation is not supported by this adapter",
        ))
    }

    /// Отправка одной MDI-команды G-code.
    fn mdi(&self, command: &str) -> CncResult<()>;

    /// Загрузить программу. Передаётся путь на стороне клиента;
    /// транспортная реализация должна передать содержимое/идентификатор
    /// согласно своему протоколу, не открывая путь на стороне сервера вслепую.
    fn load_program(&self, path: &Path) -> CncResult<()>;

    /// Запуск Auto с указанного номера строки; 0 означает начало программы.
    fn program_run(&self, start_line: u32) -> CncResult<()>;

    fn program_pause(&self) -> CncResult<()>;
    fn program_resume(&self) -> CncResult<()>;
    fn program_abort(&self) -> CncResult<()>;

    fn set_spindle(&self, command: SpindleCommand) -> CncResult<()>;

    /// Подтверждение/сброс ошибки контроллера, если поддерживается.
    fn clear_error(&self) -> CncResult<()>;
}
