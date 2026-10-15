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

use crate::cnc_api::{Axis, LinuxCncApi};
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

#[derive(Serialize)]
pub(crate) struct StatusResponse {
    connected: bool,
    state: String,
    x: f64,
    y: f64,
    z: f64,
    machine_x: f64,
    machine_y: f64,
    machine_z: f64,
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
            machine_x: status.machine_x,
            machine_y: status.machine_y,
            machine_z: status.machine_z,
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
    let controller = state
        .controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    controller
        .start_program()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| (StatusCode::BAD_REQUEST, format!("{}: {}", error.code, error.message)))
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
        || ![0.1, 1.0, 10.0].contains(&request.distance)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "direction must be -1 or 1; distance must be 0.1, 1, or 10 mm".into(),
        ));
    }

    let axis = match request.axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
    };
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
        .fallback_service(static_files)
        .with_state(controller);

    eprintln!("Web server listening on http://{address}");
    eprintln!("Warning: web controls have no authentication; use only on a trusted network.");
    axum::serve(listener, app).await
}
