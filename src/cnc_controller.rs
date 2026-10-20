//! Queues UI commands on a worker thread and maintains a cached controller status.
use crate::cnc_api::{
    Axis, CncResult, CoordinateSystem, CncApi, MachineConfiguration,
    MachineStatus, SpindleCommand, SpindleDirection as ApiSpindleDirection,
};
use cnc_core::adapters::emulator::EmAdapter;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub struct ControllerSnapshot {
    pub machine: MachineStatus,
    pub machine_configuration: Option<MachineConfiguration>,
    pub jog_step: f64,
    /// Ошибка worker для отображения в интерфейсе: (тип, текст).
    pub ui_alert: Option<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpindleDirection {
    Off,
    CW,
    CCW,
}

#[derive(Debug)]
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
    ExecuteCommand(String, mpsc::Sender<CncResult<String>>),
    Spindle(SpindleCommand),
    JogIncrement(crate::cnc_api::JogRequest),
    JogContinuous(Axis, i8, f64),
    JogContinuousXY(i8, i8, f64),
    JogCancel,
}

pub struct CNCController {
    pub status: Arc<Mutex<ControllerSnapshot>>,
    commands: mpsc::Sender<Command>,
    emergency_stop: mpsc::Sender<()>,
    xy_jog_speed: Mutex<f64>,
    z_jog_speed: Mutex<f64>,
    spindle_speed_setpoint: Mutex<f64>,
    command_cancel_requested: Arc<AtomicBool>,
    motion_stop_requested: Arc<AtomicBool>,
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
        let command_cancel_requested = Arc::new(AtomicBool::new(false));
        let worker_command_cancel_requested = command_cancel_requested.clone();
        let motion_stop_requested = Arc::new(AtomicBool::new(false));
        let worker_motion_stop_requested = motion_stop_requested.clone();

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
                                worker_status.lock().unwrap().machine = initial_status;
                                api_worker(
                                    Arc::new(api),
                                    receiver,
                                    emergency_receiver,
                                    worker_status.clone(),
                                    worker_command_cancel_requested,
                                    worker_motion_stop_requested,
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
            command_cancel_requested,
            motion_stop_requested,
        })
    }

    pub fn connect_with_api(api: Arc<dyn CncApi>) -> CncResult<Self> {
        let status = Arc::new(Mutex::new(initial_disconnected_status()));
        let worker_status = status.clone();
        let (commands, receiver) = mpsc::channel();
        let (emergency_stop, emergency_receiver) = mpsc::channel();
        let command_cancel_requested = Arc::new(AtomicBool::new(false));
        let worker_command_cancel_requested = command_cancel_requested.clone();
        let motion_stop_requested = Arc::new(AtomicBool::new(false));
        let worker_motion_stop_requested = motion_stop_requested.clone();

        thread::Builder::new()
            .name("cnc-api-worker".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create CNC API worker runtime");

                match runtime.block_on(api.status()) {
                    Ok(initial_status) => {
                        worker_status.lock().unwrap().machine = initial_status;
                    }
                    Err(error) => {
                        set_ui_alert(&worker_status, &error.code, &error.message);
                    }
                }

                api_worker(
                    api,
                    receiver,
                    emergency_receiver,
                    worker_status,
                    worker_command_cancel_requested,
                    worker_motion_stop_requested,
                    &runtime,
                );
            })
            .map_err(|error| crate::cnc_api::ControllerError::new("worker", error.to_string()))?;

        Ok(Self {
            status,
            commands,
            emergency_stop,
            xy_jog_speed: Mutex::new(1000.0),
            z_jog_speed: Mutex::new(300.0),
            spindle_speed_setpoint: Mutex::new(0.0),
            command_cancel_requested,
            motion_stop_requested,
        })
    }

    pub fn command_cancellation_handle(&self) -> Arc<AtomicBool> {
        self.command_cancel_requested.clone()
    }

    pub fn motion_stop_handle(&self) -> Arc<AtomicBool> {
        self.motion_stop_requested.clone()
    }

    pub fn cancel_machine_command(&self) {
        self.command_cancel_requested.store(true, Ordering::Release);
        self.motion_stop_requested.store(true, Ordering::Release);
    }

    fn enqueue(&self, command: Command) {
        let description = match &command {
            Command::LoadProgram(path) => format!("LoadProgram {}", path.display()),
            Command::ProgramRun => "ProgramRun".into(),
            Command::ProgramPause => "ProgramPause".into(),
            Command::ProgramResume => "ProgramResume".into(),
            Command::ProgramAbort => "ProgramAbort".into(),
            Command::ResetAlarm => "ResetAlarm".into(),
            Command::SelectCoordinateSystem(system) => {
                format!("SelectCoordinateSystem {}", system.label())
            }
            Command::SetWorkZero(system) => format!("SetWorkZero {}", system.label()),
            Command::SetAxisWorkZero(axis) => format!("SetAxisWorkZero {axis:?}"),
            Command::HomeAll => "HomeAll".into(),
            Command::Home(axis) => format!("Home {axis:?}"),
            Command::Mdi(command) => format!("MDI: {command}"),
            Command::ExecuteCommand(command, _) => format!("Команда станка: {command}"),
            Command::Spindle(command) => match command.direction {
                ApiSpindleDirection::Stopped => {
                    format!("Шпиндель: команда остановки, задано {:.0} RPM", command.rpm)
                }
                ApiSpindleDirection::Clockwise => {
                    format!("Шпиндель: включение по часовой стрелке, {:.0} RPM", command.rpm)
                }
                ApiSpindleDirection::CounterClockwise => {
                    format!("Шпиндель: включение против часовой стрелки, {:.0} RPM", command.rpm)
                }
            },
            Command::JogIncrement(request) => format!(
                "Jog: ось {:?}, направление {:+}, расстояние {:.3}, подача {:.3}",
                request.axis,
                request.direction,
                request.distance,
                request.feed_rate.unwrap_or(0.0),
            ),
            Command::JogContinuous(axis, direction, feed) => {
                if *direction == 0 {
                    format!("Jog: остановка оси {axis:?}")
                } else {
                    format!(
                        "Jog: непрерывное движение оси {axis:?}, направление {direction:+}, подача {feed:.3}"
                    )
                }
            }
            Command::JogContinuousXY(x, y, feed) => {
                if *x == 0 && *y == 0 {
                    "Jog: остановка движения XY".into()
                } else {
                    format!("Jog: непрерывное движение XY, X {x:+}, Y {y:+}, подача {feed:.3}")
                }
            }
            Command::JogCancel => "Jog: отмена движения".into(),
        };
        let source = if matches!(command, Command::Spindle(_)) {
            "шпиндель"
        } else {
            "команда UI"
        };
        crate::message_log::record("информация", source, &description);

        if let Err(error) = self.commands.send(command) {
            crate::message_log::record("ошибка", "команда UI", &format!("Команда не отправлена: {error}"));
            eprintln!("[CncApi] worker unavailable: {error}");
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
        crate::message_log::record("авария", "команда UI", "Emergency stop");
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

    pub fn execute_command(&self, command: &str) -> CncResult<String> {
        if command.trim().is_empty() || command.contains('\n') || command.contains('\r') {
            return Err(crate::cnc_api::ControllerError::new(
                "bad_command",
                "Введите одну непустую команду без переводов строк",
            ));
        }

        self.command_cancel_requested.store(false, Ordering::Release);
        let (reply_sender, reply_receiver) = mpsc::channel();
        self.enqueue(Command::ExecuteCommand(
            command.trim().to_string(),
            reply_sender,
        ));

        reply_receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|error| {
                crate::cnc_api::ControllerError::new(
                    "command_timeout",
                    format!("Нет ответа от команды станка: {error}"),
                )
            })?
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
        let spindle_direction = match snapshot.machine.spindle.direction {
            ApiSpindleDirection::Stopped => SpindleDirection::Off,
            ApiSpindleDirection::Clockwise => SpindleDirection::CW,
            ApiSpindleDirection::CounterClockwise => SpindleDirection::CCW,
        };
        if snapshot.machine.spindle.enabled && spindle_direction == direction {
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
        crate::message_log::record(
            "информация",
            "Jog",
            &format!(
                "Пошаговое движение XY: X {x_direction:+}, Y {y_direction:+}, расстояние {step:.3} mm, подача {feed:.3} mm/min",
            ),
        );
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
            'C' => Axis::C,
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
            Axis::X | Axis::Y | Axis::A | Axis::B | Axis::C => *self.xy_jog_speed.lock().unwrap(),
        };

        crate::message_log::record(
            "информация",
            "Jog",
            &format!(
                "Пошаговое движение {:?}: направление {direction:+}, расстояние {step:.3}, подача {feed_rate:.3}",
                axis,
            ),
        );
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
            'C' => Axis::C,
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
            Axis::X | Axis::Y | Axis::A | Axis::B | Axis::C => *self.xy_jog_speed.lock().unwrap(),
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
    pub fn get_status_snapshot(&self) -> ControllerSnapshot {
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

fn set_ui_alert(shared_status: &Arc<Mutex<ControllerSnapshot>>, code: &str, message: &str) {
    shared_status.lock().unwrap().ui_alert = Some(error_alert(code, message));
}

fn record_adapter_message(last_message: &mut Option<String>, severity: &str, message: &str) {
    if message.trim().is_empty() {
        *last_message = None;
        return;
    }

    let key = format!("{severity}\u{1f}{message}");
    if last_message.as_deref() != Some(key.as_str()) {
        crate::message_log::record(severity, "CNC/адаптер", message);
        *last_message = Some(key);
    }
}

fn api_worker(
    api: Arc<dyn CncApi>,
    receiver: mpsc::Receiver<Command>,
    emergency_receiver: mpsc::Receiver<()>,
    shared_status: Arc<Mutex<ControllerSnapshot>>,
    command_cancel_requested: Arc<AtomicBool>,
    motion_stop_requested: Arc<AtomicBool>,
    runtime: &tokio::runtime::Runtime,
) {
    let configuration = runtime.block_on(api.machine_configuration()).ok();
    shared_status.lock().unwrap().machine_configuration = configuration;
    let mut last_adapter_message = None;
    let mut spindle_was_active = false;
    let mut reached_spindle_target = None;
    let mut next_status_poll = Instant::now();

    loop {
        if motion_stop_requested.swap(false, Ordering::AcqRel) {
            match runtime.block_on(api.stop_motion()) {
                Ok(()) => {
                    crate::message_log::record(
                        "информация",
                        "команда UI",
                        "Остановлено текущее движение станка",
                    );
                }
                Err(error) => {
                    crate::message_log::record(
                        "ошибка",
                        "CNC/адаптер",
                        &format!("Не удалось остановить движение: {}: {}", error.code, error.message),
                    );
                    set_ui_alert(&shared_status, &error.code, &error.message);
                }
            }
        }

        // E-Stop проверяется раньше обычной очереди.
        match emergency_receiver.try_recv() {
            Ok(()) => {
                // Отбросить команды, накопившиеся до аварийной остановки.
                while receiver.try_recv().is_ok() {}
                if let Err(error) = runtime.block_on(api.estop()) {
                    eprintln!("[CncApi] E-Stop failed: {error:?}");
                    set_ui_alert(&shared_status, &error.code, &error.message);
                }
                continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(command) => {
                let command = match command {
                    Command::ExecuteCommand(command, reply) => {
                        let result = runtime.block_on(api.execute_command(
                            &command,
                            command_cancel_requested.clone(),
                        ));
                        match &result {
                            Ok(output) => {
                                crate::message_log::record(
                                    "информация",
                                    "команда станка",
                                    &format!("{command} → {output}"),
                                );
                            }
                            Err(error) => {
                                crate::message_log::record(
                                    "ошибка",
                                    "команда станка",
                                    &format!("{command}: {}: {}", error.code, error.message),
                                );
                                set_ui_alert(&shared_status, &error.code, &error.message);
                            }
                        }
                        let _ = reply.send(result);
                        continue;
                    }
                    command => command,
                };

                // E-Stop мог прийти, пока worker ожидал обычную команду.
                if emergency_receiver.try_recv().is_ok() {
                    while receiver.try_recv().is_ok() {}
                    if let Err(error) = runtime.block_on(api.estop()) {
                        eprintln!("[CncApi] E-Stop failed: {error:?}");
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
                    Command::ExecuteCommand(_, reply) => {
                        let error = crate::cnc_api::ControllerError::new(
                            "internal",
                            "command dispatch invariant violated",
                        );
                        let _ = reply.send(Err(error.clone()));
                        Err(error)
                    }
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
                        eprintln!("[CncApi] {}: {}", error.code, error.message);
                        crate::message_log::record(
                            "ошибка",
                            "CNC/адаптер",
                            &format!("{}: {}", error.code, error.message),
                        );
                        set_ui_alert(&shared_status, &error.code, &error.message);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        // Status polling at the worker's 10 ms command cadence floods FluidNC's
        // WebSocket with realtime '?' queries. Limit polling to 10 Hz instead.
        let now = Instant::now();
        if now < next_status_poll {
            continue;
        }
        next_status_poll = now + STATUS_POLL_INTERVAL;

        match runtime.block_on(api.status()) {
            Ok(status) => {
                if let Some(message) = status.message.as_deref() {
                    let severity = if status.task_state == crate::cnc_api::TaskState::Estop
                        || message.to_ascii_lowercase().starts_with("alarm")
                    {
                        "авария"
                    } else if message.to_ascii_lowercase().starts_with("error")
                        || message.to_ascii_lowercase().starts_with("ошибка")
                    {
                        "ошибка"
                    } else {
                        "информация"
                    };

                    // Обычные ответы статуса (например, "FluidNC Idle") приходят
                    // при каждом опросе и не являются событиями журнала.
                    if severity != "информация" {
                        record_adapter_message(&mut last_adapter_message, severity, message);
                    }
                } else {
                    last_adapter_message = None;
                }

                let spindle = &status.spindle;
                let spindle_active =
                    spindle.enabled || spindle.actual_rpm > 0.01 || spindle.commanded_rpm > 0.01;
                let tolerance = (spindle.commanded_rpm.abs() * 0.01).max(1.0);
                let at_target = spindle.direction != ApiSpindleDirection::Stopped
                    && spindle.commanded_rpm > 0.0
                    && (spindle.actual_rpm - spindle.commanded_rpm).abs() <= tolerance;
                let target_key = (spindle.direction, spindle.commanded_rpm.to_bits());

                if at_target && reached_spindle_target != Some(target_key) {
                    crate::message_log::record(
                        "информация",
                        "шпиндель",
                        &format!(
                            "Шпиндель достиг заданной скорости: {:.0} RPM",
                            spindle.commanded_rpm
                        ),
                    );
                    reached_spindle_target = Some(target_key);
                } else if !at_target {
                    reached_spindle_target = None;
                }

                let spindle_stopped = !spindle.enabled
                    && spindle.actual_rpm <= 0.01
                    && spindle.commanded_rpm <= 0.01;
                if spindle_was_active && spindle_stopped {
                    crate::message_log::record(
                        "информация",
                        "шпиндель",
                        "Шпиндель остановился: 0 RPM",
                    );
                    reached_spindle_target = None;
                }
                spindle_was_active = spindle_active && !spindle_stopped;

                let reconnected = !shared_status.lock().unwrap().machine.connected;
                if reconnected {
                    if let Ok(configuration) = runtime.block_on(api.machine_configuration()) {
                        shared_status.lock().unwrap().machine_configuration = Some(configuration);
                    }
                }

                let mut current = shared_status.lock().unwrap();
                current.machine = status;
                if reconnected {
                    current.machine_configuration =
                        runtime.block_on(api.machine_configuration()).ok();
                }
                if current
                    .ui_alert
                    .as_ref()
                    .is_some_and(|(kind, _)| kind == "connection")
                {
                    current.ui_alert = None;
                }
            }
            Err(error) => {
                eprintln!("[CncApi] status {}: {}", error.code, error.message);
                record_adapter_message(
                    &mut last_adapter_message,
                    "ошибка",
                    &format!("status {}: {}", error.code, error.message),
                );
                let mut current = shared_status.lock().unwrap();
                current.machine.connected = false;
                current.machine.message = Some(error.message.clone());
                current.ui_alert = Some(error_alert("connection", &error.message));
            }
        }
    }
}

fn initial_disconnected_status() -> ControllerSnapshot {
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
            active_coordinate_system: CoordinateSystem::G54,
            coordinate_systems: Vec::new(),
            feed_rate: 0.0,
            spindle: crate::cnc_api::SpindleStatus {
                commanded_rpm: 0.0,
                actual_rpm: 0.0,
                direction: ApiSpindleDirection::Stopped,
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
        },
        machine_configuration: None,
        jog_step: 1.0,
        ui_alert: Some((
            "connection".into(),
            "Waiting for CNC emulator connection".into(),
        )),
    }
}
