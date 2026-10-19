//! Queues UI commands on a worker thread and maintains a cached controller status.
use crate::cnc_api::{
    Axis, CncResult, CoordinateSystem, CoordinateSystemOffset, LinuxCncApi, MachineConfiguration,
    MachineStatus, SpindleCommand, SpindleDirection as ApiSpindleDirection,
};
use cnc_core::adapters::emulator::EmAdapter;
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
    pub a_position: f64,
    pub b_position: f64,
    pub machine_x: f64,
    pub machine_y: f64,
    pub machine_z: f64,
    pub machine_a: f64,
    pub machine_b: f64,
    pub machine_origin_x: f64,
    pub machine_origin_y: f64,
    pub machine_origin_z: f64,
    pub machine_origin_a: f64,
    pub machine_origin_b: f64,
    pub active_coordinate_system: CoordinateSystem,
    pub coordinate_systems: Vec<CoordinateSystemOffset>,
    pub machine_configuration: Option<MachineConfiguration>,
    pub feed_rate: f64,
    pub jog_rate: f64,
    pub motion_mode: Option<String>,
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
    SetAxisWorkZero(Axis),
    HomeAll,
    Home(Axis),
    Mdi(String),
    Spindle(SpindleCommand),
    JogIncrement(crate::cnc_api::JogRequest),
    JogContinuous(Axis, i8, f64),
    JogContinuousXY(i8, i8, f64),
    JogCancel,
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
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create CNC API worker runtime");

                loop {
                    match EmAdapter::connect(&socket_path) {
                        Ok(api) => match runtime.block_on(api.status()) {
                            Ok(initial_status) => {
                                *worker_status.lock().unwrap() = from_api_status(initial_status);
                                api_worker(
                                    Arc::new(api),
                                    receiver,
                                    emergency_receiver,
                                    worker_status.clone(),
                                    &runtime,
                                );
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

    pub fn connect_with_api(api: Arc<dyn LinuxCncApi>) -> CncResult<Self> {
        let status = Arc::new(Mutex::new(initial_disconnected_status()));
        let worker_status = status.clone();
        let (commands, receiver) = mpsc::channel();
        let (emergency_stop, emergency_receiver) = mpsc::channel();

        thread::Builder::new()
            .name("cnc-api-worker".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create CNC API worker runtime");

                match runtime.block_on(api.status()) {
                    Ok(initial_status) => {
                        *worker_status.lock().unwrap() = from_api_status(initial_status);
                    }
                    Err(error) => {
                        set_ui_alert(&worker_status, &error.code, &error.message);
                    }
                }

                api_worker(api, receiver, emergency_receiver, worker_status, &runtime);
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

    pub fn home_axis(&self, axis: Axis) {
        self.enqueue(Command::Home(axis));
    }

    pub fn zero_axis(&self, axis: Axis) {
        self.enqueue(Command::SetAxisWorkZero(axis));
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
            'A' => Axis::A,
            'B' => Axis::B,
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
            Axis::X | Axis::Y | Axis::A | Axis::B => *self.xy_jog_speed.lock().unwrap(),
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

    pub fn jog_axis_continuous(&self, axis: char, direction: i8) {
        let axis = match axis.to_ascii_uppercase() {
            'X' => Axis::X,
            'Y' => Axis::Y,
            'Z' => Axis::Z,
            'A' => Axis::A,
            'B' => Axis::B,
            _ => {
                eprintln!("[UI] Unsupported continuous jog axis: {axis}");
                return;
            }
        };

        if ![-1, 0, 1].contains(&direction) {
            eprintln!("[UI] Invalid continuous jog direction: {direction}");
            return;
        }

        let feed_rate = match axis {
            Axis::Z => *self.z_jog_speed.lock().unwrap(),
            Axis::X | Axis::Y | Axis::A | Axis::B => *self.xy_jog_speed.lock().unwrap(),
        };
        self.enqueue(Command::JogContinuous(axis, direction, feed_rate));
    }

    pub fn jog_xy_continuous(&self, x_direction: i8, y_direction: i8) {
        if ![-1, 0, 1].contains(&x_direction) || ![-1, 0, 1].contains(&y_direction) {
            eprintln!("[UI] Invalid continuous XY jog direction");
            return;
        }

        let feed_rate = *self.xy_jog_speed.lock().unwrap();
        self.enqueue(Command::JogContinuousXY(
            x_direction,
            y_direction,
            feed_rate,
        ));
    }

    pub fn cancel_jog(&self) {
        self.enqueue(Command::JogCancel);
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
    api: Arc<dyn LinuxCncApi>,
    receiver: mpsc::Receiver<Command>,
    emergency_receiver: mpsc::Receiver<()>,
    shared_status: Arc<Mutex<CNCStatus>>,
    runtime: &tokio::runtime::Runtime,
) {
    let configuration = runtime.block_on(api.machine_configuration()).ok();
    shared_status.lock().unwrap().machine_configuration = configuration;

    loop {
        // E-Stop проверяется раньше обычной очереди.
        match emergency_receiver.try_recv() {
            Ok(()) => {
                // Отбросить команды, накопившиеся до аварийной остановки.
                while receiver.try_recv().is_ok() {}
                if let Err(error) = runtime.block_on(api.estop()) {
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
                    if let Err(error) = runtime.block_on(api.estop()) {
                        eprintln!("[LinuxCncApi] E-Stop failed: {error:?}");
                        set_ui_alert(&shared_status, &error.code, &error.message);
                    }
                    continue;
                }
                let result = runtime.block_on(async {
                    match command {
                    Command::LoadProgram(path) => api.load_program(&path).await,
                    Command::ProgramRun => api.program_run(0).await,
                    Command::ProgramPause => api.program_pause().await,
                    Command::ProgramResume => api.program_resume().await,
                    Command::ProgramAbort => api.program_abort().await,
                    Command::ResetAlarm => match api.clear_error().await {
                        Ok(()) => api.power_on().await,
                        Err(error) => Err(error),
                    },
                    Command::SelectCoordinateSystem(system) => api.select_coordinate_system(system).await,
                    Command::SetWorkZero(system) => api.set_work_zero(system).await,
                    Command::SetAxisWorkZero(axis) => api.set_axis_work_zero(axis).await,
                    Command::HomeAll => api.home(None).await,
                    Command::Home(axis) => api.home(Some(axis)).await,
                    Command::Mdi(command) => api.mdi(&command).await,
                    Command::Spindle(command) => {
                        eprintln!(
                            "[Backend/Worker] sending spindle command to API: direction={:?}, rpm={:.0}",
                            command.direction, command.rpm
                        );
                        let result = api.set_spindle(command).await;
                        match &result {
                            Ok(()) => eprintln!("[Backend/Worker] spindle command accepted by API"),
                            Err(error) => eprintln!(
                                "[Backend/Worker] spindle command failed: {}: {}",
                                error.code, error.message
                            ),
                        }
                        result
                    }
                    Command::JogIncrement(request) => api.jog_increment(request).await,
                    Command::JogContinuous(axis, direction, feed_rate) => {
                        api.jog_continuous(axis, direction, feed_rate).await
                    }
                    Command::JogContinuousXY(x_direction, y_direction, feed_rate) => {
                        api.jog_continuous_xy(x_direction, y_direction, feed_rate).await
                    }
                    Command::JogCancel => api.cancel_jog().await,
                    }
                });

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

        match runtime.block_on(api.status()) {
            Ok(status) => {
                let reconnected = shared_status.lock().unwrap().state == "DISCONNECTED";
                if reconnected {
                    if let Ok(configuration) = runtime.block_on(api.machine_configuration()) {
                        shared_status.lock().unwrap().machine_configuration = Some(configuration);
                    }
                }

                let mut fresh = from_api_status(status);
                let mut current = shared_status.lock().unwrap();
                fresh.machine_configuration = current.machine_configuration.clone();
                if let Some((kind, _)) = current.ui_alert.as_ref() {
                    if kind != "connection" {
                        fresh.ui_alert = current.ui_alert.clone();
                    }
                }
                *current = fresh;
            }
            Err(error) => {
                eprintln!("[LinuxCncApi] status {}: {}", error.code, error.message);
                let mut current = shared_status.lock().unwrap();
                current.state = "DISCONNECTED".into();
                current.message = Some(error.message.clone());
                current.ui_alert = Some(error_alert("connection", &error.message));
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
        a_position: 0.0,
        b_position: 0.0,
        machine_x: 0.0,
        machine_y: 0.0,
        machine_z: 0.0,
        machine_a: 0.0,
        machine_b: 0.0,
        machine_origin_x: 0.0,
        machine_origin_y: 0.0,
        machine_origin_z: 0.0,
        machine_origin_a: 0.0,
        machine_origin_b: 0.0,
        active_coordinate_system: CoordinateSystem::G54,
        coordinate_systems: Vec::new(),
        machine_configuration: None,
        feed_rate: 0.0,
        jog_rate: 0.0,
        motion_mode: None,
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
        a_position: status.position.a - status.work_offset.a,
        b_position: status.position.b - status.work_offset.b,
        machine_x: status.position.x,
        machine_y: status.position.y,
        machine_z: status.position.z,
        machine_a: status.position.a,
        machine_b: status.position.b,
        machine_origin_x: -status.work_offset.x,
        machine_origin_y: -status.work_offset.y,
        machine_origin_z: -status.work_offset.z,
        machine_origin_a: -status.work_offset.a,
        machine_origin_b: -status.work_offset.b,
        active_coordinate_system: status.active_coordinate_system,
        coordinate_systems: status.coordinate_systems.clone(),
        machine_configuration: None,
        feed_rate: status.feed_rate,
        jog_rate: status.jog_rate,
        motion_mode: status.motion_mode,
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
