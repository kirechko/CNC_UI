use crate::cnc_api::{
    Axis, CncResult, CoordinateSystem, CoordinateSystemOffset,
    LinuxCncApi, MachineStatus, SpindleCommand,
    SpindleDirection as ApiSpindleDirection,
};
use crate::linuxcnc_api::LinuxCncApiClient;
use std::path::Path;
use std::sync::{Arc, Mutex};

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
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpindleDirection {
    Off,
    CW,
    CCW,
}

pub struct CNCController {
    pub status: Arc<Mutex<CNCStatus>>,
    api: LinuxCncApiClient,
    xy_jog_speed: Mutex<f64>,
    z_jog_speed: Mutex<f64>,
    spindle_speed_setpoint: Mutex<f64>,
}

#[allow(dead_code)]
impl CNCController {
    pub fn connect(socket_path: impl AsRef<Path>) -> CncResult<Self> {
        let api = LinuxCncApiClient::connect(socket_path)?;
        let initial = api.status()?;
        Ok(Self {
            status: Arc::new(Mutex::new(from_api_status(initial))),
            api,
            xy_jog_speed: Mutex::new(1000.0),
            z_jog_speed: Mutex::new(300.0),
            spindle_speed_setpoint: Mutex::new(0.0),
        })
    }

    pub fn load_program(&self, path: &Path) -> CncResult<()> {
        self.api.load_program(path)?;
        self.refresh_status()
    }

    pub fn program_run(&self) -> CncResult<()> {
        self.api.program_run(0)?;
        self.refresh_status()
    }

    pub fn program_pause(&self) -> CncResult<()> {
        self.api.program_pause()?;
        self.refresh_status()
    }

    pub fn program_resume(&self) -> CncResult<()> {
        self.api.program_resume()?;
        self.refresh_status()
    }

    pub fn program_abort(&self) -> CncResult<()> {
        self.api.program_abort()?;
        self.refresh_status()
    }


    pub fn start_program(&self) -> CncResult<()> {
        self.api.program_run(0)?;
        self.refresh_status()
    }

    pub fn emergency_stop(&self) {
        if let Err(error) = self.api.estop() {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        }
        let _ = self.refresh_status();
    }

    pub fn reset_alarm(&self) {
        if let Err(error) = self.api.clear_error() {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        } else if let Err(error) = self.api.power_on() {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        }
        let _ = self.refresh_status();
    }

    pub fn select_coordinate_system(&self, system: CoordinateSystem) {
        if let Err(error) = self.api.select_coordinate_system(system) {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        }
        let _ = self.refresh_status();
    }

    pub fn set_work_zero(&self, system: CoordinateSystem) {
        if let Err(error) = self.api.set_work_zero(system) {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        }
        let _ = self.refresh_status();
    }

    pub fn home_all(&self) {
        if let Err(error) = self.api.home(None) {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        }
        let _ = self.refresh_status();
    }

    fn refresh_status(&self) -> CncResult<()> {
        let snapshot = self.api.status()?;
        *self.status.lock().unwrap() = from_api_status(snapshot);
        Ok(())
    }

    pub fn send_command(&self, command: &str) {
        if let Err(error) = self.api.mdi(command) {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        }
        let _ = self.refresh_status();
    }

    pub fn set_spindle_speed(&self, speed: f64) {
        if !speed.is_finite() || speed < 0.0 {
            eprintln!("[UI] Ignored invalid spindle speed: {speed}");
            return;
        }

        // Слайдер меняет только выбранную скорость.
        // В работающий шпиндель новая уставка не отправляется.
        *self.spindle_speed_setpoint.lock().unwrap() = speed;
    }

    pub fn set_spindle_direction(&self, direction: SpindleDirection) {
        if direction == SpindleDirection::Off {
            self.stop_spindle();
            return;
        }

        let current = self.status.lock().unwrap().clone();
        if current.spindle_on && current.spindle_direction == direction {
            // Повторное нажатие того же направления ничего не меняет.
            return;
        }

        let rpm = *self.spindle_speed_setpoint.lock().unwrap();
        if rpm <= 0.0 {
            eprintln!("[UI] Set spindle speed above zero before starting.");
            return;
        }

        let api_direction = match direction {
            SpindleDirection::CW => ApiSpindleDirection::Clockwise,
            SpindleDirection::CCW => ApiSpindleDirection::CounterClockwise,
            SpindleDirection::Off => ApiSpindleDirection::Stopped,
        };

        self.send_spindle(api_direction, rpm);
    }

    fn send_spindle(&self, direction: ApiSpindleDirection, rpm: f64) {
        if let Err(error) = self.api.set_spindle(SpindleCommand { direction, rpm }) {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        }
        let _ = self.refresh_status();
    }

    pub fn stop_spindle(&self) {
        self.send_spindle(ApiSpindleDirection::Stopped, 0.0);
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

    /// Одновременный диагональный jog XY одной согласованной командой.
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
        let x = step * f64::from(x_direction);
        let y = step * f64::from(y_direction);
        let command = format!("G91 G1 X{x:.3} Y{y:.3} F{feed:.0}");

        if let Err(error) = self.api.mdi(&command) {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        }
        // Возвращаем модальный абсолютный режим; команда не содержит движения осей.
        if let Err(error) = self.api.mdi("G90") {
            eprintln!("[LinuxCncApi] G90 restore: {}: {}", error.code, error.message);
        }
        let _ = self.refresh_status();
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

        let request = crate::cnc_api::JogRequest {
            axis,
            direction,
            distance: step,
            feed_rate: Some(feed_rate),
        };

        if let Err(error) = self.api.jog_increment(request) {
            eprintln!("[LinuxCncApi] {}: {}", error.code, error.message);
        }
        let _ = self.refresh_status();
    }

    pub fn jog_axis(&self, axis: char, direction: i8) {
        let step = self.status.lock().unwrap().jog_step;
        self.jog_axis_step(axis, direction, step);
    }

    pub fn get_status_snapshot(&self) -> CNCStatus {
        if let Err(error) = self.refresh_status() {
            eprintln!("[LinuxCncApi] status {}: {}", error.code, error.message);
        }
        self.status.lock().unwrap().clone()
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
        // Панель и инструмент показывают рабочие координаты.
        x_position: status.position.x - status.work_offset.x,
        y_position: status.position.y - status.work_offset.y,
        z_position: status.position.z - status.work_offset.z,
        machine_x: status.position.x,
        machine_y: status.position.y,
        machine_z: status.position.z,
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
    }
}
