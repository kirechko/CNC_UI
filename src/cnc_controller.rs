use crate::cnc_api::{
    Axis, CncResult, CoordinateSystem, CoordinateSystemOffset, LinuxCncApi, MachineStatus,
    SpindleCommand, SpindleDirection as ApiSpindleDirection,
};
use crate::linuxcnc_api::LinuxCncApiClient;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CNCStatus {
    pub mode: String,
    pub state: String,
    pub x_position: f64,
    pub y_position: f64,
    pub z_position: f64,
    pub machine_x: f64,
    pub machine_y: f64,
    pub machine_z: f64,
    pub machine_origin_x: f64,
    pub machine_origin_y: f64,
    pub machine_origin_z: f64,
    pub active_coordinate_system: CoordinateSystem,
    pub coordinate_systems: Vec<CoordinateSystemOffset>,
    pub feed_rate: f64,
    pub spindle_speed: f64,
    pub spindle_actual_speed: f64,
    pub spindle_max_speed: f64,
    pub mist_coolant_on: bool,
    pub flood_coolant_on: bool,
    pub message: Option<String>,
    pub spindle_on: bool,
    pub spindle_direction: SpindleDirection,
    pub jog_step: f64,
    pub current_line: u32,
    pub total_lines: u32,
    /// Ошибка worker для отображения в интерфейсе: (тип, текст).
    pub ui_alert: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpindleDirection {
    Off,
    CW,
    CCW,
}

enum Command {
    LoadProgram(PathBuf),
    ProgramRun,
    ProgramPause,
    ProgramResume,
    ProgramAbort,
    ResetAlarm,
    SelectCoordinateSystem(CoordinateSystem),
    SetWorkZero(CoordinateSystem),
    HomeAll,
    Mdi(String),
    Spindle(SpindleCommand),
    JogIncrement(crate::cnc_api::JogRequest),
}

pub struct CNCController {
    pub status: Arc<Mutex<CNCStatus>>,
    commands: mpsc::Sender<Command>,
    emergency_stop: mpsc::Sender<()>,
    xy_jog_speed: Mutex<f64>,
    z_jog_speed: Mutex<f64>,
    spindle_speed_setpoint: Mutex<f64>,
}

#[allow(dead_code)]
impl CNCController {
    pub fn connect(socket_path: impl AsRef<Path>) -> CncResult<Self> {
        // Создаём UI-controller без сетевых вызовов. Подключение и status poll
        // выполняются только в worker, поэтому окно может открыться без эмулятора.
        let socket_path = socket_path.as_ref().to_path_buf();
        let status = Arc::new(Mutex::new(initial_disconnected_status()));
        let worker_status = status.clone();
        let (commands, receiver) = mpsc::channel();
        let (emergency_stop, emergency_receiver) = mpsc::channel();

        thread::Builder::new()
            .name("cnc-api-worker".into())
            .spawn(move || loop {
                match LinuxCncApiClient::connect(&socket_path) {
                    Ok(api) => match api.status() {
                        Ok(initial_status) => {
                            *worker_status.lock().unwrap() = from_api_status(initial_status);
                            api_worker(api, receiver, emergency_receiver, worker_status.clone());
                            return;
                        }
                        Err(error) => {
                            set_ui_alert(&worker_status, &error.code, &error.message);
                            thread::sleep(Duration::from_secs(1));
                        }
                    },
                    Err(error) => {
                        set_ui_alert(&worker_status, &error.code, &error.message);
                        thread::sleep(Duration::from_secs(1));
                    }
                }
            })
            .map_err(|error| crate::cnc_api::ControllerError::new("worker", error.to_string()))?;

        Ok(Self {
            status,
            commands,
            emergency_stop,
            xy_jog_speed: Mutex::new(1000.0),
            z_jog_speed: Mutex::new(300.0),
            spindle_speed_setpoint: Mutex::new(0.0),
        })
    }

    fn enqueue(&self, command: Command) {
        if let Err(error) = self.commands.send(command) {
            eprintln!("[LinuxCncApi] worker unavailable: {error}");
        }
    }

    pub fn load_program(&self, path: &Path) -> CncResult<()> {
        self.enqueue(Command::LoadProgram(path.to_path_buf()));
        Ok(())
    }

    pub fn program_run(&self) -> CncResult<()> {
        self.enqueue(Command::ProgramRun);
        Ok(())
    }

    pub fn start_program(&self) -> CncResult<()> {
        self.program_run()
    }

    pub fn program_pause(&self) -> CncResult<()> {
        self.enqueue(Command::ProgramPause);
        Ok(())
    }

    pub fn program_resume(&self) -> CncResult<()> {
        self.enqueue(Command::ProgramResume);
        Ok(())
    }

    pub fn program_abort(&self) -> CncResult<()> {
        self.enqueue(Command::ProgramAbort);
        Ok(())
    }

    pub fn emergency_stop(&self) {
        if let Err(error) = self.emergency_stop.send(()) {
            eprintln!("[LinuxCncApi] emergency-stop worker unavailable: {error}");
        }
    }

    pub fn reset_alarm(&self) {
        self.enqueue(Command::ResetAlarm);
    }

    pub fn select_coordinate_system(&self, system: CoordinateSystem) {
        self.enqueue(Command::SelectCoordinateSystem(system));
    }

    pub fn set_work_zero(&self, system: CoordinateSystem) {
        self.enqueue(Command::SetWorkZero(system));
    }

    pub fn home_all(&self) {
        self.enqueue(Command::HomeAll);
    }

    pub fn send_command(&self, command: &str) {
        self.enqueue(Command::Mdi(command.to_string()));
    }

    pub fn set_spindle_speed(&self, speed: f64) {
        if speed.is_finite() && speed >= 0.0 {
            *self.spindle_speed_setpoint.lock().unwrap() = speed;
        } else {
            eprintln!("[UI] Ignored invalid spindle speed: {speed}");
        }
    }

    pub fn set_spindle_direction(&self, direction: SpindleDirection) {
        if direction == SpindleDirection::Off {
            self.stop_spindle();
            return;
        }

        let snapshot = self.get_status_snapshot();
        if snapshot.spindle_on && snapshot.spindle_direction == direction {
            return;
        }

        let rpm = *self.spindle_speed_setpoint.lock().unwrap();
        if rpm <= 0.0 {
            eprintln!("[UI] Set spindle speed above zero before starting.");
            return;
        }

        let direction = match direction {
            SpindleDirection::CW => ApiSpindleDirection::Clockwise,
            SpindleDirection::CCW => ApiSpindleDirection::CounterClockwise,
            SpindleDirection::Off => ApiSpindleDirection::Stopped,
        };
        self.enqueue(Command::Spindle(SpindleCommand { direction, rpm }));
    }

    pub fn stop_spindle(&self) {
        self.enqueue(Command::Spindle(SpindleCommand {
            direction: ApiSpindleDirection::Stopped,
            rpm: 0.0,
        }));
    }

    pub fn set_xy_jog_speed(&self, speed: f64) {
        if speed.is_finite() && speed > 0.0 {
            *self.xy_jog_speed.lock().unwrap() = speed;
        }
    }

    pub fn set_z_jog_speed(&self, speed: f64) {
        if speed.is_finite() && speed > 0.0 {
            *self.z_jog_speed.lock().unwrap() = speed;
        }
    }

    pub fn set_jog_step(&self, step: f64) {
        if step.is_finite() && step > 0.0 {
            self.status.lock().unwrap().jog_step = step;
        }
    }

    pub fn jog_xy_step(&self, x_direction: i8, y_direction: i8, step: f64) {
        if !step.is_finite()
            || step <= 0.0
            || ![-1, 1].contains(&x_direction)
            || ![-1, 1].contains(&y_direction)
        {
            eprintln!("[UI] Invalid XY jog request");
            return;
        }

        let feed = *self.xy_jog_speed.lock().unwrap();
        self.enqueue(Command::Mdi(format!(
            "G91 G1 X{:.3} Y{:.3} F{feed:.0}",
            step * f64::from(x_direction),
            step * f64::from(y_direction),
        )));
        self.enqueue(Command::Mdi("G90".into()));
    }

    pub fn jog_axis_step(&self, axis: char, direction: i8, step: f64) {
        let axis = match axis.to_ascii_uppercase() {
            'X' => Axis::X,
            'Y' => Axis::Y,
            'Z' => Axis::Z,
            _ => {
                eprintln!("[UI] Unsupported jog axis: {axis}");
                return;
            }
        };

        if !step.is_finite() || step <= 0.0 || ![-1, 1].contains(&direction) {
            eprintln!("[UI] Invalid jog request");
            return;
        }

        let feed_rate = match axis {
            Axis::Z => *self.z_jog_speed.lock().unwrap(),
            Axis::X | Axis::Y => *self.xy_jog_speed.lock().unwrap(),
        };

        self.enqueue(Command::JogIncrement(crate::cnc_api::JogRequest {
            axis,
            direction,
            distance: step,
            feed_rate: Some(feed_rate),
        }));
    }

    pub fn jog_axis(&self, axis: char, direction: i8) {
        let step = self.status.lock().unwrap().jog_step;
        self.jog_axis_step(axis, direction, step);
    }

    /// Возвращает кэшированный снимок без сетевого запроса.
    pub fn get_status_snapshot(&self) -> CNCStatus {
        self.status.lock().unwrap().clone()
    }
}

fn error_alert(code: &str, message: &str) -> (String, String) {
    let kind = match code {
        "soft_limit" => "axis_limit",
        "connect" | "connection" | "io" | "socket" | "disconnected" => "connection",
        _ => "command_rejected",
    };
    (kind.to_string(), message.to_string())
}

fn set_ui_alert(shared_status: &Arc<Mutex<CNCStatus>>, code: &str, message: &str) {
    shared_status.lock().unwrap().ui_alert = Some(error_alert(code, message));
}

fn api_worker(
    api: LinuxCncApiClient,
    receiver: mpsc::Receiver<Command>,
    emergency_receiver: mpsc::Receiver<()>,
    shared_status: Arc<Mutex<CNCStatus>>,
) {
    loop {
        // E-Stop проверяется раньше обычной очереди.
        match emergency_receiver.try_recv() {
            Ok(()) => {
                // Отбросить команды, накопившиеся до аварийной остановки.
                while receiver.try_recv().is_ok() {}
                if let Err(error) = api.estop() {
                    eprintln!("[LinuxCncApi] E-Stop failed: {error:?}");
                    set_ui_alert(&shared_status, &error.code, &error.message);
                }
                continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(command) => {
                // E-Stop мог прийти, пока worker ожидал обычную команду.
                if emergency_receiver.try_recv().is_ok() {
                    while receiver.try_recv().is_ok() {}
                    if let Err(error) = api.estop() {
                        eprintln!("[LinuxCncApi] E-Stop failed: {error:?}");
                        set_ui_alert(&shared_status, &error.code, &error.message);
                    }
                    continue;
                }
                let result = match command {
                    Command::LoadProgram(path) => api.load_program(&path),
                    Command::ProgramRun => api.program_run(0),
                    Command::ProgramPause => api.program_pause(),
                    Command::ProgramResume => api.program_resume(),
                    Command::ProgramAbort => api.program_abort(),
                    Command::ResetAlarm => api.clear_error().and_then(|_| api.power_on()),
                    Command::SelectCoordinateSystem(system) => api.select_coordinate_system(system),
                    Command::SetWorkZero(system) => api.set_work_zero(system),
                    Command::HomeAll => api.home(None),
                    Command::Mdi(command) => api.mdi(&command),
                    Command::Spindle(command) => api.set_spindle(command),
                    Command::JogIncrement(request) => api.jog_increment(request),
                };

                match result {
                    Ok(()) => {
                        shared_status.lock().unwrap().ui_alert = None;
                    }
                    Err(error) => {
                        eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
                        set_ui_alert(&shared_status, &error.code, &error.message);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        match api.status() {
            Ok(status) => {
                let mut fresh = from_api_status(status);
                let mut current = shared_status.lock().unwrap();
                if let Some((kind, _)) = current.ui_alert.as_ref() {
                    if kind != "connection" {
                        fresh.ui_alert = current.ui_alert.clone();
                    }
                }
                *current = fresh;
            }
            Err(error) => {
                eprintln!("[LinuxCncApi] status {}: {}", error.code, error.message);
                set_ui_alert(&shared_status, "connection", &error.message);
            }
        }
    }
}

fn initial_disconnected_status() -> CNCStatus {
    CNCStatus {
        mode: "MANUAL".into(),
        state: "DISCONNECTED".into(),
        x_position: 0.0,
        y_position: 0.0,
        z_position: 0.0,
        machine_x: 0.0,
        machine_y: 0.0,
        machine_z: 0.0,
        machine_origin_x: 0.0,
        machine_origin_y: 0.0,
        machine_origin_z: 0.0,
        active_coordinate_system: CoordinateSystem::G54,
        coordinate_systems: Vec::new(),
        feed_rate: 0.0,
        spindle_speed: 0.0,
        spindle_actual_speed: 0.0,
        spindle_max_speed: 0.0,
        mist_coolant_on: false,
        flood_coolant_on: false,
        message: Some("Waiting for CNC emulator connection".into()),
        spindle_on: false,
        spindle_direction: SpindleDirection::Off,
        jog_step: 1.0,
        current_line: 0,
        total_lines: 0,
        ui_alert: Some((
            "connection".into(),
            "Waiting for CNC emulator connection".into(),
        )),
    }
}

fn from_api_status(status: MachineStatus) -> CNCStatus {
    let spindle_direction = match status.spindle.direction {
        ApiSpindleDirection::Stopped => SpindleDirection::Off,
        ApiSpindleDirection::Clockwise => SpindleDirection::CW,
        ApiSpindleDirection::CounterClockwise => SpindleDirection::CCW,
    };

    CNCStatus {
        mode: format!("{:?}", status.task_mode).to_uppercase(),
        state: if status.task_state == crate::cnc_api::TaskState::Estop {
            "ALARM".into()
        } else if status.homing {
            "HOMING".into()
        } else {
            format!("{:?}", status.interpreter_state).to_uppercase()
        },
        x_position: status.position.x - status.work_offset.x,
        y_position: status.position.y - status.work_offset.y,
        z_position: status.position.z - status.work_offset.z,
        machine_x: status.position.x,
        machine_y: status.position.y,
        machine_z: status.position.z,
        machine_origin_x: -status.work_offset.x,
        machine_origin_y: -status.work_offset.y,
        machine_origin_z: -status.work_offset.z,
        active_coordinate_system: status.active_coordinate_system,
        coordinate_systems: status.coordinate_systems.clone(),
        feed_rate: status.feed_rate,
        spindle_speed: status.spindle.commanded_rpm,
        spindle_actual_speed: status.spindle.actual_rpm,
        spindle_max_speed: status.max_spindle_rpm,
        mist_coolant_on: status.mist_coolant_on,
        flood_coolant_on: status.flood_coolant_on,
        message: status.message,
        spindle_on: status.spindle.enabled,
        spindle_direction,
        jog_step: 1.0,
        current_line: status.current_line,
        total_lines: status.total_lines,
        ui_alert: None,
    }
}
