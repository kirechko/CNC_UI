use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tower_http::services::{ServeDir, ServeFile};

use crate::cnc_api::{
    Axis, InterpreterState, LinuxCncApi, MachineConfiguration, TaskMode, TaskState,
};
use crate::cnc_controller::{CNCController, ControllerSnapshot};

const DEFAULT_ADDRESS: &str = "0.0.0.0:8080";
const WEB_ROOT: &str = "web";

type SharedController = Arc<AppState>;
static UPLOADED_PROGRAM_ID: AtomicU64 = AtomicU64::new(0);
const MAX_PROGRAM_BYTES: usize = 5 * 1024 * 1024;
const DEFAULT_LOG_PAGE_SIZE: usize = 100;
const MAX_LOG_PAGE_SIZE: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LogEntry {
    pub(crate) id: i64,
    pub(crate) timestamp_ms: i64,
    pub(crate) severity: String,
    pub(crate) source: String,
    pub(crate) message: String,
}

#[derive(Serialize)]
pub(crate) struct LatestLogMessages {
    pub(crate) critical: Option<LogEntry>,
    pub(crate) other: Option<LogEntry>,
}

#[derive(Deserialize)]
pub(crate) struct LogQuery {
    pub(crate) severity: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) search: Option<String>,
    pub(crate) sort: Option<String>,
    pub(crate) descending: Option<bool>,
    pub(crate) limit: Option<usize>,
    pub(crate) offset: Option<usize>,
    pub(crate) time_from_ms: Option<i64>,
    pub(crate) time_to_ms: Option<i64>,
}

#[derive(Serialize)]
pub(crate) struct LogPage {
    pub(crate) entries: Vec<LogEntry>,
    pub(crate) total: i64,
}

#[derive(Clone, Serialize)]
pub(crate) struct LoadedProgram {
    pub(crate) name: String,
    pub(crate) content: String,
}

pub(crate) struct AppState {
    pub(crate) controller: Mutex<CNCController>,
    pub(crate) program: Mutex<Option<LoadedProgram>>,
    log: Mutex<Connection>,
    last_logged_status: Mutex<Option<String>>,
    command_cancel_requested: Arc<std::sync::atomic::AtomicBool>,
    motion_stop_requested: Arc<std::sync::atomic::AtomicBool>,
}

impl AppState {
    fn open_log() -> io::Result<Connection> {
        let path = std::env::var_os("CNC_UI_LOG_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let state_dir = std::env::var_os("XDG_STATE_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
                        std::env::var_os("HOME")
                            .map(PathBuf::from)
                            .map(|home| home.join(".local").join("state"))
                    })
                    .unwrap_or_else(std::env::temp_dir);
                state_dir
                    .join("linuxcnc-ui")
                    .join("messages.sqlite3")
            });

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let connection = Connection::open(&path)
            .map_err(|error| startup_error(format!("cannot open message log {}: {error}", path.display())))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE IF NOT EXISTS messages (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     timestamp_ms INTEGER NOT NULL,
                     severity TEXT NOT NULL,
                     source TEXT NOT NULL,
                     message TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS messages_severity_id
                     ON messages(severity, id DESC);
                 CREATE INDEX IF NOT EXISTS messages_id
                     ON messages(id DESC);
                 CREATE TABLE IF NOT EXISTS command_history (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     command TEXT NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS command_history_id
                     ON command_history(id DESC);",
            )
            .map_err(|error| startup_error(format!("cannot initialize message log: {error}")))?;
        Ok(connection)
    }

    pub(crate) fn record_status(&self, status: &ControllerSnapshot) {
        let alert = status.ui_alert.as_ref().map(|(kind, message)| (kind, message));
        let key = format!(
            "{}\u{1f}{}\u{1f}{}",
            status_state(status),
            alert.map_or("", |(kind, _)| kind.as_str()),
            alert.map_or("", |(_, message)| message.as_str()),
        );

        let Ok(mut previous) = self.last_logged_status.lock() else {
            return;
        };
        if previous.as_deref() == Some(&key) {
            return;
        }
        *previous = Some(key);
        drop(previous);

        if let Some((kind, message)) = alert {
            let severity = match (status.state.as_str(), kind.as_str()) {
                ("ALARM", _) => "авария",
                (_, "axis_limit") => "предупреждение",
                _ => "ошибка",
            };
            self.insert_log(severity, "контроллер", message);
        }
    }

    pub(crate) fn insert_log(&self, severity: &str, source: &str, message: &str) {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;

        if let Ok(connection) = self.log.lock() {
            if let Err(error) = connection.execute(
                "INSERT INTO messages (timestamp_ms, severity, source, message)
                 VALUES (?1, ?2, ?3, ?4)",
                params![timestamp_ms, severity, source, message],
            ) {
                eprintln!("Could not write message log: {error}");
            }
        }
    }

    pub(crate) fn latest_log_messages(&self) -> Result<LatestLogMessages, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "message log lock poisoned".to_string())?;
        let critical = connection
            .query_row(
                "SELECT id, timestamp_ms, severity, source, message
                 FROM messages WHERE severity = 'авария' ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok(LogEntry {
                        id: row.get(0)?,
                        timestamp_ms: row.get(1)?,
                        severity: row.get(2)?,
                        source: row.get(3)?,
                        message: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let other = connection
            .query_row(
                "SELECT id, timestamp_ms, severity, source, message
                 FROM messages WHERE severity != 'авария' ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok(LogEntry {
                        id: row.get(0)?,
                        timestamp_ms: row.get(1)?,
                        severity: row.get(2)?,
                        source: row.get(3)?,
                        message: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;

        Ok(LatestLogMessages { critical, other })
    }

    pub(crate) fn query_log(&self, query: &LogQuery) -> Result<LogPage, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "message log lock poisoned".to_string())?;
        let severity = query.severity.as_deref().filter(|value| !value.is_empty());
        let source = query.source.as_deref().filter(|value| !value.is_empty());
        let search = query.search.as_deref().filter(|value| !value.is_empty());
        let time_from_ms = query.time_from_ms;
        let time_to_ms = query.time_to_ms;
        let limit = query
            .limit
            .unwrap_or(DEFAULT_LOG_PAGE_SIZE)
            .clamp(1, MAX_LOG_PAGE_SIZE);
        let offset = query.offset.unwrap_or(0);
        let order = match query.sort.as_deref() {
            Some("severity") => "severity",
            Some("source") => "source",
            Some("message") => "message",
            Some("timestamp") => "timestamp_ms",
            _ => "id",
        };
        let direction = if query.descending.unwrap_or(true) {
            "DESC"
        } else {
            "ASC"
        };

        let total = connection
            .query_row(
                "SELECT COUNT(*) FROM messages
                 WHERE (?1 IS NULL OR severity = ?1)
                   AND (?2 IS NULL OR instr(source, ?2) > 0)
                   AND (?3 IS NULL OR message LIKE '%' || ?3 || '%')
                   AND (?4 IS NULL OR timestamp_ms >= ?4)
                   AND (?5 IS NULL OR timestamp_ms <= ?5)",
                params![severity, source, search, time_from_ms, time_to_ms],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let sql = format!(
            "SELECT id, timestamp_ms, severity, source, message FROM messages
             WHERE (?1 IS NULL OR severity = ?1)
               AND (?2 IS NULL OR instr(source, ?2) > 0)
               AND (?3 IS NULL OR message LIKE '%' || ?3 || '%')
               AND (?4 IS NULL OR timestamp_ms >= ?4)
               AND (?5 IS NULL OR timestamp_ms <= ?5)
             ORDER BY {order} {direction}, id DESC LIMIT ?6 OFFSET ?7"
        );
        let mut statement = connection
            .prepare(&sql)
            .map_err(|error| error.to_string())?;
        let entries = statement
            .query_map(
                params![
                    severity,
                    source,
                    search,
                    time_from_ms,
                    time_to_ms,
                    limit,
                    offset
                ],
                |row| {
                Ok(LogEntry {
                    id: row.get(0)?,
                    timestamp_ms: row.get(1)?,
                    severity: row.get(2)?,
                    source: row.get(3)?,
                    message: row.get(4)?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;

        Ok(LogPage { entries, total })
    }
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
            return Err(
                "Целевая координата должна быть конечной и находиться в диапазоне ±100000".into(),
            );
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
            return Err(
                "Целевая координата должна быть конечной и находиться в диапазоне ±100000".into(),
            );
        }

        let system = parse_coordinate_system(key)?;
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        let status = controller.get_status_snapshot();
        if !status.machine.connected {
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

    pub(crate) fn command_history(&self) -> Result<Vec<String>, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "message log lock poisoned".to_string())?;
        let mut statement = connection
            .prepare(
                "SELECT command FROM (
                     SELECT id, command FROM command_history
                     ORDER BY id DESC LIMIT 500
                 ) ORDER BY id ASC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        let commands = rows
            .collect::<Result<Vec<String>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(commands)
    }

    pub(crate) fn cancel_machine_command(&self) {
        self.command_cancel_requested.store(
            true,
            std::sync::atomic::Ordering::Release,
        );
        self.motion_stop_requested.store(
            true,
            std::sync::atomic::Ordering::Release,
        );
    }

    pub(crate) fn execute_command(&self, command: &str) -> Result<String, String> {
        let command = command.trim();
        if command.is_empty()
            || command.len() > 4096
            || command.contains('\n')
            || command.contains('\r')
        {
            return Err("Некорректная команда: укажите одну строку длиной до 4096 символов".into());
        }

        if let Ok(connection) = self.log.lock() {
            let result = connection.execute(
                "INSERT INTO command_history (command)
                 SELECT ?1
                 WHERE NOT EXISTS (
                     SELECT 1 FROM command_history
                     ORDER BY id DESC LIMIT 1
                 ) OR (
                     SELECT command FROM command_history
                     ORDER BY id DESC LIMIT 1
                 ) != ?1",
                [command],
            );
            if let Err(error) = result {
                eprintln!("Could not store command history: {error}");
            } else if let Err(error) = connection.execute(
                "DELETE FROM command_history
                 WHERE id NOT IN (
                     SELECT id FROM command_history ORDER BY id DESC LIMIT 500
                 )",
                [],
            ) {
                eprintln!("Could not prune command history: {error}");
            }
        }

        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller
            .execute_command(command)
            .map_err(|error| format!("{}: {}", error.code, error.message))
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
        let controller = create_controller()?;
        let command_cancel_requested = controller.command_cancellation_handle();
        let motion_stop_requested = controller.motion_stop_handle();
        let state = Self {
            controller: Mutex::new(controller),
            program: Mutex::new(None),
            log: Mutex::new(Self::open_log()?),
            last_logged_status: Mutex::new(None),
            command_cancel_requested,
            motion_stop_requested,
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
                        eprintln!(
                            "Loaded default G-code trajectory from {}",
                            default_path.display()
                        );
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                eprintln!(
                    "Default G-code file {} not found; starting without a trajectory",
                    default_path.display()
                );
            }
            Err(error) => {
                eprintln!(
                    "Could not read default G-code file {}: {error}",
                    default_path.display()
                );
            }
        }

        Ok(state)
    }
}

#[derive(Deserialize)]
struct MachineCommandRequest {
    command: String,
}

#[derive(Serialize)]
struct MachineCommandResponse {
    output: String,
}

#[derive(Deserialize)]
struct LogMessageRequest {
    severity: String,
    source: String,
    message: String,
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
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Не удалось создать каталог программы: {error}"))?;
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
struct SpindleRequest {
    rpm: f64,
    enabled: bool,
    direction: String,
}

#[derive(Deserialize)]
struct JogRequest {
    axis: Axis,
    direction: i8,
    distance: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct XyJogRequest {
    x_direction: i8,
    y_direction: i8,
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

async fn set_spindle(
    State(state): State<SharedController>,
    Json(request): Json<SpindleRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    eprintln!(
        "[Backend/HTTP] set_spindle received: enabled={}, direction={}, rpm={}",
        request.enabled, request.direction, request.rpm
    );

    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;

    if !request.enabled {
        controller.stop_spindle();
        return Ok(StatusCode::ACCEPTED);
    }

    if !request.rpm.is_finite() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Скорость шпинделя должна быть конечным числом".into(),
        ));
    }

    let status = controller.get_status_snapshot();
    if !status.machine.connected {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Нет соединения со станком".into(),
        ));
    }

    let spindle = status
        .machine_configuration
        .as_ref()
        .map(|configuration| &configuration.spindle)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "Конфигурация шпинделя недоступна".into(),
            )
        })?;
    let minimum = spindle.min_rpm.unwrap_or(0.0);
    let maximum = spindle.max_rpm.unwrap_or(status.spindle_max_speed);

    if maximum <= 0.0 || request.rpm < minimum || request.rpm > maximum {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Скорость шпинделя должна быть в диапазоне {minimum:.0}–{maximum:.0} RPM"),
        ));
    }

    let direction = match request.direction.as_str() {
        "CW" => crate::cnc_controller::SpindleDirection::CW,
        "CCW" => crate::cnc_controller::SpindleDirection::CCW,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Направление шпинделя должно быть CW или CCW".into(),
            ));
        }
    };

    controller.set_spindle_speed(request.rpm);
    controller.set_spindle_direction(direction);
    Ok(StatusCode::ACCEPTED)
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

    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    let axis = axis_letter(request.axis);
    controller.jog_axis_continuous(axis, request.direction);
    Ok(StatusCode::ACCEPTED)
}

async fn jog_cancel(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller.cancel_jog();
    Ok(StatusCode::ACCEPTED)
}

async fn jog_continuous_xy(
    State(state): State<SharedController>,
    Json(request): Json<ContinuousXyJogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if ![-1, 0, 1].contains(&request.x_direction) || ![-1, 0, 1].contains(&request.y_direction) {
        return Err((
            StatusCode::BAD_REQUEST,
            "XY directions must be -1, 0, or 1".into(),
        ));
    }

    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
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
    movement_speed: f64,
    movement_mode: Option<String>,
    spindle_rpm: f64,
    spindle_commanded_rpm: f64,
    spindle_enabled: bool,
    spindle_direction: String,
    spindle_min_rpm: f64,
    spindle_max_rpm: f64,
    current_line: u32,
    total_lines: u32,
    message: Option<String>,
    error: Option<String>,
    pub(crate) latest_critical: Option<LogEntry>,
    pub(crate) latest_other: Option<LogEntry>,
}

impl From<ControllerSnapshot> for StatusResponse {
    fn from(status: ControllerSnapshot) -> Self {
        let machine = status.machine;
        let spindle_min_rpm = status
            .machine_configuration
            .as_ref()
            .and_then(|configuration| configuration.spindle.min_rpm)
            .unwrap_or(0.0);
        let spindle_max_rpm = status
            .machine_configuration
            .as_ref()
            .and_then(|configuration| configuration.spindle.max_rpm)
            .unwrap_or(machine.max_spindle_rpm);

        let state = if machine.task_state == TaskState::Estop {
            "ALARM"
        } else if machine.homing {
            "HOMING"
        } else if !machine.connected {
            "DISCONNECTED"
        } else {
            match machine.interpreter_state {
                InterpreterState::Idle => "IDLE",
                InterpreterState::Reading => "READING",
                InterpreterState::Paused => "PAUSED",
                InterpreterState::Waiting => "WAITING",
            }
        };
        let mode = match machine.task_mode {
            TaskMode::Manual => "MANUAL",
            TaskMode::Mdi => "MDI",
            TaskMode::Auto => "AUTO",
        };

        Self {
            connected: machine.connected,
            state: state.into(),
            x: machine.position.x - machine.work_offset.x,
            y: machine.position.y - machine.work_offset.y,
            z: machine.position.z - machine.work_offset.z,
            a: machine.position.a - machine.work_offset.a,
            b: machine.position.b - machine.work_offset.b,
            machine_x: machine.position.x,
            machine_y: machine.position.y,
            machine_z: machine.position.z,
            machine_a: machine.position.a,
            machine_b: machine.position.b,
            coordinate_system: machine
                .active_coordinate_system
                .label()
                .to_ascii_lowercase()
                .replace('.', "_"),
            configuration: status.machine_configuration,
            movement_speed: if machine.jog_rate > 0.0 {
                machine.jog_rate
            } else {
                machine.feed_rate
            },
            movement_mode: machine.motion_mode,
            spindle_rpm: machine.spindle.actual_rpm,
            spindle_commanded_rpm: machine.spindle.commanded_rpm,
            spindle_enabled: machine.spindle.enabled,
            spindle_direction: match machine.spindle.direction {
                crate::cnc_api::SpindleDirection::Stopped => "stopped",
                crate::cnc_api::SpindleDirection::Clockwise => "cw",
                crate::cnc_api::SpindleDirection::CounterClockwise => "ccw",
            }
            .into(),
            spindle_min_rpm,
            spindle_max_rpm,
            current_line: machine.current_line,
            total_lines: machine.total_lines,
            message: machine.message,
            error: status
                .ui_alert
                .map(|(kind, message)| format!("{kind}: {message}")),
            latest_critical: None,
            latest_other: None,
        }
    }
}

fn status_state(status: &ControllerSnapshot) -> &'static str {
    let machine = &status.machine;
    if machine.task_state == TaskState::Estop {
        "ALARM"
    } else if machine.homing {
        "HOMING"
    } else if !machine.connected {
        "DISCONNECTED"
    } else {
        match machine.interpreter_state {
            InterpreterState::Idle => "IDLE",
            InterpreterState::Reading => "READING",
            InterpreterState::Paused => "PAUSED",
            InterpreterState::Waiting => "WAITING",
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
                cnc_core::adapters::emulator::EmAdapter::connect(&socket_path)
                    .map_err(|error| startup_error(format!("{}: {}", error.code, error.message)))?,
            )
        }
        crate::config::Backend::LinuxCnc => {
            Arc::new(cnc_core::adapters::debug::LinuxCncAdapter::new())
        }
        crate::config::Backend::Grbl => Arc::new(
            cnc_core::adapters::grbl::GrblAdapter::connect(
                &config.grbl.serial_port,
                config.grbl.baud_rate,
            )
            .map_err(|error| startup_error(format!("{}: {}", error.code, error.message)))?,
        ),
        crate::config::Backend::FluidNc => {
            Arc::new(cnc_core::adapters::debug::FluidNcAdapter::new())
        }
    };

    CNCController::connect_with_api(api)
        .map_err(|error| startup_error(format!("{}: {}", error.code, error.message)))
}

async fn get_status(
    State(state): State<SharedController>,
) -> Result<Json<StatusResponse>, (StatusCode, String)> {
    let status = {
        let controller = state.controller.lock().map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "controller lock poisoned".into(),
            )
        })?;
        controller.get_status_snapshot()
    };
    state.record_status(&status);
    let mut response = StatusResponse::from(status);
    if let Ok(latest) = state.latest_log_messages() {
        response.latest_critical = latest.critical;
        response.latest_other = latest.other;
    }
    Ok(Json(response))
}

async fn cancel_machine_command(
    State(state): State<SharedController>,
) -> StatusCode {
    state.cancel_machine_command();
    StatusCode::ACCEPTED
}

async fn get_command_history(
    State(state): State<SharedController>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    state
        .command_history()
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

async fn get_log(
    State(state): State<SharedController>,
    Query(query): Query<LogQuery>,
) -> Result<Json<LogPage>, (StatusCode, String)> {
    state
        .query_log(&query)
        .map(Json)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error))
}

async fn post_machine_command(
    State(state): State<SharedController>,
    Json(request): Json<MachineCommandRequest>,
) -> Result<Json<MachineCommandResponse>, (StatusCode, String)> {
    let command = request.command.trim();
    if command.is_empty()
        || command.len() > 4096
        || command.contains('\n')
        || command.contains('\r')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Введите одну непустую команду длиной не более 4096 символов".into(),
        ));
    }

    state
        .execute_command(command)
        .map(|output| Json(MachineCommandResponse { output }))
        .map_err(|error| (StatusCode::BAD_REQUEST, error))
}

async fn post_log_message(
    State(state): State<SharedController>,
    Json(request): Json<LogMessageRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    let severity = request.severity.trim();
    let source = request.source.trim();
    let message = request.message.trim();

    if !["авария", "ошибка", "предупреждение", "информация"].contains(&severity) {
        return Err((StatusCode::BAD_REQUEST, "Неизвестная критичность".into()));
    }
    if source.is_empty() || source.len() > 128 || message.is_empty() || message.len() > 16_384 {
        return Err((StatusCode::BAD_REQUEST, "Некорректный источник или сообщение".into()));
    }

    state.insert_log(severity, source, message);
    Ok(StatusCode::CREATED)
}

async fn get_program(
    State(state): State<SharedController>,
) -> Result<Json<Option<LoadedProgram>>, (StatusCode, String)> {
    let program = state.program.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "program lock poisoned".into(),
        )
    })?;
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
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller
        .program_pause()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                format!("{}: {}", error.code, error.message),
            )
        })
}

async fn resume_program(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller
        .program_resume()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                format!("{}: {}", error.code, error.message),
            )
        })
}

async fn abort_program(
    State(state): State<SharedController>,
) -> Result<StatusCode, (StatusCode, String)> {
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller
        .program_abort()
        .map(|_| StatusCode::ACCEPTED)
        .map_err(|error| {
            (
                StatusCode::BAD_REQUEST,
                format!("{}: {}", error.code, error.message),
            )
        })
}

async fn jog_xy(
    State(state): State<SharedController>,
    Json(request): Json<XyJogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if ![-1, 1].contains(&request.x_direction)
        || ![-1, 1].contains(&request.y_direction)
        || !request.distance.is_finite()
        || request.distance <= 0.0
        || request.distance > 100_000.0
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "XY directions must be -1 or 1; distance must be greater than 0 and at most 100000 mm"
                .into(),
        ));
    }

    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
    controller.jog_xy_step(request.x_direction, request.y_direction, request.distance);
    Ok(StatusCode::ACCEPTED)
}

async fn jog(
    State(state): State<SharedController>,
    Json(request): Json<JogRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    if ![-1, 1].contains(&request.direction)
        || !request.distance.is_finite()
        || request.distance <= 0.0
        || request.distance > 100_000.0
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "direction must be -1 or 1; distance must be greater than 0 and at most 100000 mm"
                .into(),
        ));
    }

    let axis = axis_letter(request.axis);
    let controller = state.controller.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "controller lock poisoned".into(),
        )
    })?;
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
        .route("/api/log", get(get_log).post(post_log_message))
        .route("/api/command", post(post_machine_command))
        .route("/api/command/cancel", post(cancel_machine_command))
        .route("/api/command-history", get(get_command_history))
        .route("/api/program", get(get_program).post(post_program))
        .route("/api/program/run", post(start_program))
        .route("/api/program/pause", post(pause_program))
        .route("/api/program/resume", post(resume_program))
        .route("/api/program/abort", post(abort_program))
        .route("/api/jog", post(jog))
        .route("/api/spindle", post(set_spindle))
        .route("/api/jog/xy", post(jog_xy))
        .route("/api/jog/continuous", post(jog_continuous))
        .route("/api/jog/continuous-xy", post(jog_continuous_xy))
        .route("/api/jog/cancel", post(jog_cancel))
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

#[cfg(test)]
mod contract_tests {
    use super::*;

    fn sample_status() -> ControllerSnapshot {
        ControllerSnapshot {
            machine: crate::cnc_api::MachineStatus {
                connected: true,
                task_mode: TaskMode::Manual,
                task_state: TaskState::On,
                interpreter_state: InterpreterState::Idle,
                position: crate::cnc_api::Position {
                    x: 11.0,
                    y: 12.0,
                    z: 13.0,
                    a: 14.0,
                    b: 15.0,
                },
                work_offset: crate::cnc_api::Position {
                    x: 10.0,
                    y: 10.0,
                    z: 10.0,
                    a: 10.0,
                    b: 10.0,
                },
                active_coordinate_system: crate::cnc_api::CoordinateSystem::G59_1,
                coordinate_systems: Vec::new(),
                feed_rate: 100.0,
                spindle: crate::cnc_api::SpindleStatus {
                    commanded_rpm: 0.0,
                    actual_rpm: 0.0,
                    direction: crate::cnc_api::SpindleDirection::Stopped,
                    enabled: false,
                },
                mist_coolant_on: false,
                flood_coolant_on: false,
                jog_rate: 0.0,
                motion_mode: None,
                max_spindle_rpm: 24_000.0,
                loaded_program: None,
                current_line: 0,
                total_lines: 0,
                message: Some("ready".into()),
                homed: [false; 5],
                homing: false,
            },
            machine_configuration: None,
            jog_step: 1.0,
            ui_alert: None,
        }
    }

    #[test]
    fn status_response_preserves_frontend_json_field_names() {
        let value = serde_json::to_value(StatusResponse::from(sample_status())).unwrap();

        for field in [
            "connected",
            "state",
            "x",
            "y",
            "z",
            "a",
            "b",
            "machine_x",
            "machine_y",
            "machine_z",
            "machine_a",
            "machine_b",
            "coordinate_system",
            "configuration",
            "movement_speed",
            "movement_mode",
            "spindle_rpm",
            "spindle_commanded_rpm",
            "spindle_enabled",
            "spindle_direction",
            "spindle_min_rpm",
            "spindle_max_rpm",
            "current_line",
            "total_lines",
            "message",
            "error",
        ] {
            assert!(value.get(field).is_some(), "missing frontend field {field}");
        }

        assert_eq!(value["coordinate_system"], "g59_1");
        assert_eq!(value["x"], 1.0);
        assert_eq!(value["machine_x"], 11.0);
    }

    #[test]
    fn frontend_json_request_names_deserialize_unchanged() {
        let program: ProgramRequest = serde_json::from_value(serde_json::json!({
            "fileName": "part.ngc",
            "content": "G0 X1"
        }))
        .unwrap();
        assert_eq!(program.file_name, "part.ngc");

        let jog: XyJogRequest = serde_json::from_value(serde_json::json!({
            "xDirection": 1,
            "yDirection": -1,
            "distance": 0.1
        }))
        .unwrap();
        assert_eq!((jog.x_direction, jog.y_direction), (1, -1));

        let continuous_jog: ContinuousXyJogRequest = serde_json::from_value(serde_json::json!({
            "xDirection": 1,
            "yDirection": 0
        }))
        .unwrap();
        assert_eq!(
            (continuous_jog.x_direction, continuous_jog.y_direction),
            (1, 0)
        );
    }

    #[test]
    fn loaded_program_keeps_frontend_json_shape() {
        let value = serde_json::to_value(LoadedProgram {
            name: "part.ngc".into(),
            content: "G0 X1".into(),
        })
        .unwrap();

        assert_eq!(
            value,
            serde_json::json!({
                "name": "part.ngc",
                "content": "G0 X1"
            })
        );
    }
}
