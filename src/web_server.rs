use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

use crate::cnc_api::{Axis, LinuxCncApi, MachineConfiguration};
use crate::cnc_controller::{CNCController, CNCStatus};

const DEFAULT_ADDRESS: &str = "0.0.0.0:8080";
const WEB_ROOT: &str = "web";

type SharedController = Arc<AppState>;
static UPLOADED_PROGRAM_ID: AtomicU64 = AtomicU64::new(0);
const MAX_PROGRAM_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Serialize)]
pub(crate) struct LoadedProgram {
    pub(crate) name: String,
    pub(crate) content: String,
}

pub(crate) struct AppState {
    pub(crate) controller: Mutex<CNCController>,
    pub(crate) program: Mutex<Option<LoadedProgram>>,
}

fn parse_coordinate_system(value: &str) -> Result<crate::cnc_api::CoordinateSystem, String> {
    use crate::cnc_api::CoordinateSystem::*;

    match value {
        "g54" | "G54" => Ok(G54),
        "g55" | "G55" => Ok(G55),
        "g56" | "G56" => Ok(G56),
        "g57" | "G57" => Ok(G57),
        "g58" | "G58" => Ok(G58),
        "g59" | "G59" => Ok(G59),
        "g59_1" | "G59.1" => Ok(G59_1),
        "g59_2" | "G59.2" => Ok(G59_2),
        "g59_3" | "G59.3" => Ok(G59_3),
        _ => Err(format!("Неизвестная система координат: {value}")),
    }
}

fn coordinate_system_number(system: crate::cnc_api::CoordinateSystem) -> u8 {
    use crate::cnc_api::CoordinateSystem::*;

    match system {
        G54 => 1,
        G55 => 2,
        G56 => 3,
        G57 => 4,
        G58 => 5,
        G59 => 6,
        G59_1 => 7,
        G59_2 => 8,
        G59_3 => 9,
    }
}

fn axis_letter(axis: Axis) -> char {
    match axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
        Axis::A => 'A',
        Axis::B => 'B',
    }
}

impl AppState {
    pub(crate) fn select_coordinate_system(&self, key: &str) -> Result<(), String> {
        let system = parse_coordinate_system(key)?;
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.select_coordinate_system(system);
        Ok(())
    }

    pub(crate) fn zero_all(&self, key: &str) -> Result<(), String> {
        let system = parse_coordinate_system(key)?;
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.set_work_zero(system);
        Ok(())
    }

    pub(crate) fn zero_axis(&self, axis: Axis) -> Result<(), String> {
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.zero_axis(axis);
        Ok(())
    }

    pub(crate) fn home_axis(&self, axis: Axis) -> Result<(), String> {
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.home_axis(axis);
        Ok(())
    }

    pub(crate) fn home_all(&self) -> Result<(), String> {
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.home_all();
        Ok(())
    }

    pub(crate) fn set_axis_work_value(
        &self,
        axis: Axis,
        key: &str,
        value: f64,
    ) -> Result<(), String> {
        if !value.is_finite() || value.abs() > 100_000.0 {
            return Err("Целевая координата должна быть конечной и находиться в диапазоне ±100000".into());
        }

        let system = parse_coordinate_system(key)?;
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        if controller.get_status_snapshot().state == "DISCONNECTED" {
            return Err("Нет соединения со станком".into());
        }

        let command = format!(
            "G10 L2 P{} {}{value:.6}",
            coordinate_system_number(system),
            axis_letter(axis)
        );
        controller.send_command(&command);
        Ok(())
    }

    pub(crate) fn go_to_axis_work_value(
        &self,
        axis: Axis,
        key: &str,
        value: f64,
    ) -> Result<(), String> {
        if !value.is_finite() || value.abs() > 100_000.0 {
            return Err("Целевая координата должна быть конечной и находиться в диапазоне ±100000".into());
        }

        let system = parse_coordinate_system(key)?;
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        let status = controller.get_status_snapshot();
        if status.state == "DISCONNECTED" {
            return Err("Нет соединения со станком".into());
        }

        let feed_rate = if status.feed_rate.is_finite() && status.feed_rate > 0.0 {
            status.feed_rate
        } else {
            100.0
        };
        let command = format!(
            "{} G90 G1 {}{value:.6} F{feed_rate:.0}",
            system.label(),
            axis_letter(axis)
        );
        controller.send_command(&command);
        Ok(())
    }

    pub(crate) fn start_loaded_program(&self) -> Result<(), String> {
        let program = self
            .program
            .lock()
            .map_err(|_| "program lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "Программа не загружена".to_string())?;

        if !program
            .content
            .lines()
            .any(|line| !line.split(';').next().unwrap_or("").trim().is_empty())
        {
            return Err("Загруженная программа не содержит команд".into());
        }

        let directory = std::env::temp_dir().join("cnc-ui-programs");
        fs::create_dir_all(&directory)
            .map_err(|error| format!("Не удалось создать каталог программы: {error}"))?;
        let id = UPLOADED_PROGRAM_ID.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("program-{}-{id}.ngc", std::process::id()));
        fs::write(&path, &program.content)
            .map_err(|error| format!("Не удалось восстановить G-code: {error}"))?;

        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller
            .load_program(&path)
            .map_err(|error| format!("{}: {}", error.code, error.message))?;
        controller
            .start_program()
            .map_err(|error| format!("{}: {}", error.code, error.message))
    }
}

impl AppState {
    pub(crate) fn new() -> io::Result<Self> {
        let state = Self {
            controller: Mutex::new(create_controller()?),
            program: Mutex::new(None),
        };
        let default_path = PathBuf::from("test.gcode");

        match fs::read_to_string(&default_path) {
            Ok(content) => {
                if let Ok(controller) = state.controller.lock() {
                    if let Err(error) = controller.load_program(&default_path) {
                        eprintln!(
                            "Could not load default program {}: {}: {}",
                            default_path.display(),
                            error.code,
                            error.message
                        );
                    } else if let Ok(mut program) = state.program.lock() {
                        *program = Some(LoadedProgram {
                            name: default_path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("test.gcode")
                                .to_string(),
                            content,
                        });
                        eprintln!("Loaded default G-code trajectory from {}", default_path.display());
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                eprintln!("Default G-code file {} not found; starting without a trajectory", default_path.display());
            }
            Err(error) => {
                eprintln!("Could not read default G-code file {}: {error}", default_path.display());
            }
        }

        Ok(state)
    }
}

#[derive(Deserialize)]
struct ProgramRequest {
    #[serde(rename = "fileName")]
    file_name: String,
    content: String,
}

fn validate_program(name: &str, content: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > 255 || name.contains(['/', '\\']) {
        return Err("Имя файла должно быть именем файла без пути и не длиннее 255 символов".into());
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["nc", "ngc", "gcode"].contains(&extension.as_str()) {
        return Err("Поддерживаются файлы .nc, .ngc и .gcode".into());
    }
    if content.len() > MAX_PROGRAM_BYTES {
        return Err("Размер G-code превышает лимит 5 MiB".into());
    }
    Ok(())
}

pub(crate) fn load_program(
    state: &AppState,
    name: &str,
    content: &str,
) -> Result<LoadedProgram, String> {
    validate_program(name, content)?;

    let directory = std::env::temp_dir().join("cnc-ui-programs");
    fs::create_dir_all(&directory).map_err(|error| format!("Не удалось создать каталог программы: {error}"))?;
    let id = UPLOADED_PROGRAM_ID.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(format!("program-{}-{id}.ngc", std::process::id()));
    fs::write(&path, content).map_err(|error| format!("Не удалось сохранить G-code: {error}"))?;

    let controller = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    controller
        .load_program(&path)
        .map_err(|error| format!("{}: {}", error.code, error.message))?;

    let program = LoadedProgram {
        name: name.to_string(),
        content: content.to_string(),
    };
    *state
        .program
        .lock()
        .map_err(|_| "program lock poisoned".to_string())? = Some(program.clone());

    Ok(program)
}

#[derive(Deserialize)]
struct JogRequest {
    axis: Axis,
    direction: i8,
    distance: f64,
}

#[derive(Deserialize)]
struct ContinuousJogRequest {
    axis: Axis,
    direction: i8,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContinuousXyJogRequest {
    x_direction: i8,
    y_direction: i8,
}

#[derive(Deserialize)]
struct CoordinateRequest {
    system: String,
}

#[derive(Deserialize)]
struct AxisRequest {
    axis: Axis,
}

#[derive(Deserialize)]
struct AxisPositionRequest {
    axis: Axis,
    system: String,
    value: f64,
}

async fn jog_continuous(
    State(state): State<SharedController>,
    Json(request): Json<ContinuousJogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if ![-1, 0, 1].contains(&request.direction) {
        return Err((
            StatusCode::BAD_REQUEST,
            "direction must be -1, 0, or 1".into(),
        ));
    }

    let controller = state
        .controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    let axis = axis_letter(request.axis);
    controller.jog_axis_continuous(axis, request.direction);
    Ok(StatusCode::ACCEPTED)
}

async fn jog_continuous_xy(
    State(state): State<SharedController>,
    Json(request): Json<ContinuousXyJogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if ![-1, 0, 1].contains(&request.x_direction)
        || ![-1, 0, 1].contains(&request.y_direction)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "XY directions must be -1, 0, or 1".into(),
        ));
    }

    let controller = state
        .controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    controller.jog_xy_continuous(request.x_direction, request.y_direction);
    Ok(StatusCode::ACCEPTED)
}

async fn select_coordinate_system(
    State(state): State<SharedController>,
    Json(request): Json<CoordinateRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .select_coordinate_system(&request.system)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn zero_all(
    State(state): State<SharedController>,
    Json(request): Json<CoordinateRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .zero_all(&request.system)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn zero_axis(
    State(state): State<SharedController>,
    Json(request): Json<AxisRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .zero_axis(request.axis)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn home_axis(
    State(state): State<SharedController>,
    Json(request): Json<AxisRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .home_axis(request.axis)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn home_all(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .home_all()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn set_axis_work_value(
    State(state): State<SharedController>,
    Json(request): Json<AxisPositionRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .set_axis_work_value(request.axis, &request.system, request.value)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn go_to_axis_work_value(
    State(state): State<SharedController>,
    Json(request): Json<AxisPositionRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .go_to_axis_work_value(request.axis, &request.system, request.value)
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

#[derive(Serialize)]
pub(crate) struct StatusResponse {
    connected: bool,
    state: String,
    x: f64,
    y: f64,
    z: f64,
    a: f64,
    b: f64,
    machine_x: f64,
    machine_y: f64,
    machine_z: f64,
    machine_a: f64,
    machine_b: f64,
    coordinate_system: String,
    configuration: Option<MachineConfiguration>,
    feed_rate: f64,
    spindle_rpm: f64,
    current_line: u32,
    total_lines: u32,
    message: Option<String>,
    error: Option<String>,
}

impl From<CNCStatus> for StatusResponse {
    fn from(status: CNCStatus) -> Self {
        Self {
            connected: status.state != "DISCONNECTED",
            state: status.state,
            x: status.x_position,
            y: status.y_position,
            z: status.z_position,
            a: status.a_position,
            b: status.b_position,
            machine_x: status.machine_x,
            machine_y: status.machine_y,
            machine_z: status.machine_z,
            machine_a: status.machine_a,
            machine_b: status.machine_b,
            coordinate_system: status.active_coordinate_system.label().to_ascii_lowercase().replace('.', "_"),
            configuration: status.machine_configuration,
            feed_rate: status.feed_rate,
            spindle_rpm: status.spindle_actual_speed,
            current_line: status.current_line,
            total_lines: status.total_lines,
            message: status.message,
            error: status
                .ui_alert
                .map(|(kind, message)| format!("{kind}: {message}")),
        }
    }
}

fn startup_error(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, message.into())
}

pub(crate) fn create_controller() -> io::Result<CNCController> {
    let config_path = std::env::var_os("CNC_UI_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("cnc-ui.yaml"));
    let config = crate::config::AppConfig::load(&config_path)
        .map_err(|error| startup_error(format!("configuration error: {error}")))?;

    let api: Arc<dyn LinuxCncApi> = match config.backend {
        crate::config::Backend::Emulator => {
            let socket_path = std::env::var_os("CNC_EMULATOR_SOCKET")
                .map(PathBuf::from)
                .unwrap_or_else(|| config.emulator.socket_path.clone().into());
            Arc::new(
                crate::em_adapter::EmAdapter::connect(&socket_path)
                    .map_err(|error| startup_error(format!("{}: {}", error.code, error.message)))?,
            )
        }
        crate::config::Backend::LinuxCnc => {
            Arc::new(crate::adapters::LinuxCncAdapter::new())
        }
        crate::config::Backend::Grbl => Arc::new(
            crate::grbl_adapter::GrblAdapter::connect(
                &config.grbl.serial_port,
                config.grbl.baud_rate,
            )
            .map_err(|error| startup_error(format!("{}: {}", error.code, error.message)))?,
        ),
        crate::config::Backend::FluidNc => {
            Arc::new(crate::adapters::FluidNcAdapter::new())
        }
    };

    CNCController::connect_with_api(api)
        .map_err(|error| startup_error(format!("{}: {}", error.code, error.message)))
}

async fn get_status(
    State(state): State<SharedController>,
) -> Result<Json<StatusResponse>, (StatusCode, String)> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    Ok(Json(controller.get_status_snapshot().into()))
}

async fn get_program(
    State(state): State<SharedController>,
) -> Result<Json<Option<LoadedProgram>>, (StatusCode, String)> {
    let program = state
        .program
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "program lock poisoned".into()))?;
    Ok(Json(program.clone()))
}

async fn post_program(
    State(state): State<SharedController>,
    Json(request): Json<ProgramRequest>,
) -> Result<Json<LoadedProgram>, (StatusCode, String)> {
    load_program(&state, &request.file_name, &request.content)
        .map(Json)
        .map_err(|error| {
            let status = if error.starts_with("Не удалось") {
                StatusCode::INTERNAL_SERVER_ERROR
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, error)
        })
}

async fn start_program(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .start_loaded_program()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn pause_program(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    controller
        .program_pause()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, format!("{}: {}", error.code, error.message)))
}

async fn resume_program(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    controller
        .program_resume()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, format!("{}: {}", error.code, error.message)))
}

async fn abort_program(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    let controller = state
        .controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    controller
        .program_abort()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, format!("{}: {}", error.code, error.message)))
}

async fn jog(
    State(state): State<SharedController>,
    Json(request): Json<JogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if ![-1, 1].contains(&request.direction)
        || ![0.1, 1.0, 10.0, 100.0].contains(&request.distance)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "direction must be -1 or 1; distance must be 0.1, 1, 10, or 100 mm".into(),
        ));
    }

    let axis = axis_letter(request.axis);
    let controller = state
        .controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    controller.jog_axis_step(axis, request.direction, request.distance);
    Ok(StatusCode::ACCEPTED)
}

pub async fn run() -> io::Result<()> {
    let controller = Arc::new(AppState::new()?);
    let listener = TcpListener::bind(DEFAULT_ADDRESS).await?;
    let address = listener.local_addr()?;

    let static_files =
        ServeDir::new(WEB_ROOT).fallback(ServeFile::new(format!("{WEB_ROOT}/index.html")));
    let app = Router::new()
        .route("/api/status", get(get_status))
        .route("/api/program", get(get_program).post(post_program))
        .route("/api/program/run", post(start_program))
        .route("/api/program/pause", post(pause_program))
        .route("/api/program/resume", post(resume_program))
        .route("/api/program/abort", post(abort_program))
        .route("/api/jog", post(jog))
        .route("/api/jog/continuous", post(jog_continuous))
        .route("/api/jog/continuous-xy", post(jog_continuous_xy))
        .route("/api/position/select", post(select_coordinate_system))
        .route("/api/position/zero", post(zero_all))
        .route("/api/position/zero-axis", post(zero_axis))
        .route("/api/position/home", post(home_axis))
        .route("/api/position/home-all", post(home_all))
        .route("/api/position/set", post(set_axis_work_value))
        .route("/api/position/go", post(go_to_axis_work_value))
        .fallback_service(static_files)
        .with_state(controller);

    eprintln!("Web server listening on http://{address}");
    eprintln!("Warning: web controls have no authentication; use only on a trusted network.");
    axum::serve(listener, app).await
}
