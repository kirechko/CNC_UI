//! Implements the CNC API client using a persistent Unix-socket connection.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use tokio::time::timeout;

#[derive(Serialize)]
struct Request<'a> {
    method: &'a str,
    args: Value,
}

#[derive(Deserialize)]
struct Response {
    ok: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<RemoteError>,
}

#[derive(Deserialize)]
struct RemoteError {
    code: String,
    message: String,
}

struct ApiConnection {
    reader: BufReader<UnixStream>,
}

/// Адаптер эмулятора: преобразует общий CNC API в запросы по Unix-сокету.
/// Использует собственный JSON-протокол эмулятора, а не NML.
pub struct EmAdapter {
    socket_path: PathBuf,
    timeout: Duration,
    connection: Mutex<Option<ApiConnection>>,
}

impl EmAdapter {
    pub fn connect(path: impl AsRef<Path>) -> CncResult<Self> {
        let path = path.as_ref().to_path_buf();
        Ok(Self {
            socket_path: path,
            timeout: Duration::from_secs(3),
            connection: Mutex::new(None),
        })
    }

    #[allow(dead_code)]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    async fn call(&self, method: &str, args: Value) -> CncResult<Value> {
        let mut slot = self.connection.lock().await;

        if slot.is_none() {
            let stream = timeout(self.timeout, UnixStream::connect(&self.socket_path))
                .await
                .map_err(|_| ControllerError::new("timeout", "timed out connecting to emulator"))?
                .map_err(|error| ControllerError::new("connect", error.to_string()))?;
            *slot = Some(ApiConnection {
                reader: BufReader::new(stream),
            });
        }

        let result = match timeout(self.timeout, async {
            let connection = slot.as_mut().expect("connection was initialized");
            let request = serde_json::to_vec(&Request { method, args })
                .map_err(|error| ControllerError::new("serialize", error.to_string()))?;

            connection
                .reader
                .get_mut()
                .write_all(&request)
                .await
                .map_err(|error| ControllerError::new("io", error.to_string()))?;
            connection
                .reader
                .get_mut()
                .write_all(b"\n")
                .await
                .map_err(|error| ControllerError::new("io", error.to_string()))?;

            let mut line = String::new();
            let bytes_read = connection
                .reader
                .read_line(&mut line)
                .await
                .map_err(|error| ControllerError::new("io", error.to_string()))?;
            if bytes_read == 0 {
                return Err(ControllerError::new(
                    "disconnected",
                    "emulator closed connection",
                ));
            }

            let response: Response = serde_json::from_str(&line)
                .map_err(|error| ControllerError::new("protocol", error.to_string()))?;
            if !response.ok {
                let error = response.error.unwrap_or(RemoteError {
                    code: "unknown".into(),
                    message: "emulator returned an unspecified error".into(),
                });
                return Err(ControllerError::new(error.code, error.message));
            }

            Ok(response.result)
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for emulator response",
            )),
        };

        if result.is_err() {
            *slot = None;
        }
        result
    }

    async fn unit(&self, method: &str, args: Value) -> CncResult<()> {
        self.call(method, args).await.map(|_| ())
    }
}

#[async_trait::async_trait]
impl LinuxCncApi for EmAdapter {
    async fn status(&self) -> CncResult<MachineStatus> {
        let value = self.call("status", json!({})).await?;
        serde_json::from_value(value).map_err(|e| ControllerError::new("protocol", e.to_string()))
    }

    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let value = self.call("machine_configuration", json!({})).await?;
        serde_json::from_value(value).map_err(|e| ControllerError::new("protocol", e.to_string()))
    }

    async fn power_on(&self) -> CncResult<()> {
        self.unit("power_on", json!({})).await
    }

    async fn power_off(&self) -> CncResult<()> {
        self.unit("power_off", json!({})).await
    }

    async fn estop(&self) -> CncResult<()> {
        self.unit("estop", json!({})).await
    }

    async fn home(&self, axis: Option<Axis>) -> CncResult<()> {
        self.unit("home", json!({ "axis": axis })).await
    }

    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        self.unit("select_coordinate_system", json!({ "system": system }))
            .await
    }

    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        self.unit("set_work_zero", json!({ "system": system }))
            .await
    }

    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        self.unit("set_axis_work_zero", json!({ "axis": axis }))
            .await
    }

    async fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        self.unit("jog_increment", json!({ "request": request }))
            .await
    }

    async fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()> {
        self.unit(
            "jog_continuous",
            json!({ "axis": axis, "direction": direction, "feed_rate": feed_rate }),
        )
        .await
    }

    async fn jog_continuous_xy(
        &self,
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        self.unit(
            "jog_continuous_xy",
            json!({
                "x_direction": x_direction,
                "y_direction": y_direction,
                "feed_rate": feed_rate
            }),
        )
        .await
    }

    async fn cancel_jog(&self) -> CncResult<()> {
        self.unit("jog_cancel", json!({})).await
    }

    async fn stop_motion(&self) -> CncResult<()> {
        self.unit("stop_motion", json!({})).await
    }

    async fn execute_command(
        &self,
        command: &str,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> CncResult<String> {
        if cancellation.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ControllerError::new("command_cancelled", "Команда отменена."));
        }
        self.unit("mdi", json!({ "command": command })).await?;
        if cancellation.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ControllerError::new("command_cancelled", "Команда отменена."));
        }
        Ok("Команда принята эмулятором.".into())
    }

    async fn mdi(&self, command: &str) -> CncResult<()> {
        self.unit("mdi", json!({ "command": command })).await
    }

    async fn load_program(&self, path: &Path) -> CncResult<()> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("program.ngc");
        self.unit("load_program", json!({ "name": name, "content": content }))
            .await
    }

    async fn program_run(&self, start_line: u32) -> CncResult<()> {
        self.unit("program_run", json!({ "start_line": start_line }))
            .await
    }

    async fn program_pause(&self) -> CncResult<()> {
        self.unit("program_pause", json!({})).await
    }

    async fn program_resume(&self) -> CncResult<()> {
        self.unit("program_resume", json!({})).await
    }

    async fn program_abort(&self) -> CncResult<()> {
        self.unit("program_abort", json!({})).await
    }

    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        self.unit("set_spindle", json!({ "command": command }))
            .await
    }

    async fn clear_error(&self) -> CncResult<()> {
        self.unit("clear_error", json!({})).await
    }
}
