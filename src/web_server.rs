use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

use crate::cnc_api::{Axis, LinuxCncApi};
use crate::cnc_controller::{CNCController, CNCStatus};

const DEFAULT_ADDRESS: &str = "0.0.0.0:8080";
const WEB_ROOT: &str = "web";

type SharedController = Arc<Mutex<CNCController>>;

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
    State(controller): State<SharedController>,
) -> Result<Json<StatusResponse>, (StatusCode, String)> {
    let controller = controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    Ok(Json(controller.get_status_snapshot().into()))
}

async fn jog(
    State(controller): State<SharedController>,
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
    let controller = controller
        .lock()
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "controller lock poisoned".into()))?;
    controller.jog_axis_step(axis, request.direction, request.distance);
    Ok(StatusCode::ACCEPTED)
}

pub async fn run() -> io::Result<()> {
    let controller = Arc::new(Mutex::new(create_controller()?));
    let listener = TcpListener::bind(DEFAULT_ADDRESS).await?;
    let address = listener.local_addr()?;

    let static_files =
        ServeDir::new(WEB_ROOT).fallback(ServeFile::new(format!("{WEB_ROOT}/index.html")));
    let app = Router::new()
        .route("/api/status", get(get_status))
        .route("/api/jog", post(jog))
        .fallback_service(static_files)
        .with_state(controller);

    eprintln!("Web server listening on http://{address}");
    eprintln!("Warning: web controls have no authentication; use only on a trusted network.");
    axum::serve(listener, app).await
}
