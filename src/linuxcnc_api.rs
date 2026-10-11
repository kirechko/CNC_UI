use crate::cnc_api::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// Клиент собственного LinuxCNC-подобного API поверх Unix-сокета.
/// Протокол не является NML и рассчитан на этот UI и cnc-emulator.
pub struct LinuxCncApiClient {
    socket_path: PathBuf,
    timeout: Duration,
}

impl LinuxCncApiClient {
    pub fn connect(path: impl AsRef<Path>) -> CncResult<Self> {
        let path = path.as_ref().to_path_buf();
        UnixStream::connect(&path)
            .map_err(|e| ControllerError::new("connect", format!("{}: {e}", path.display())))?;
        Ok(Self {
            socket_path: path,
            timeout: Duration::from_secs(3),
        })
    }

    #[allow(dead_code)]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    fn call(&self, method: &str, args: Value) -> CncResult<Value> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| ControllerError::new("connect", e.to_string()))?;
        stream.set_read_timeout(Some(self.timeout))
            .map_err(|e| ControllerError::new("socket", e.to_string()))?;
        stream.set_write_timeout(Some(self.timeout))
            .map_err(|e| ControllerError::new("socket", e.to_string()))?;

        let request = serde_json::to_vec(&Request { method, args })
            .map_err(|e| ControllerError::new("serialize", e.to_string()))?;
        stream.write_all(&request)
            .and_then(|_| stream.write_all(b"\n"))
            .map_err(|e| ControllerError::new("io", e.to_string()))?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line)
            .map_err(|e| ControllerError::new("io", e.to_string()))?;
        if line.is_empty() {
            return Err(ControllerError::new("disconnected", "emulator closed connection"));
        }

        let response: Response = serde_json::from_str(&line)
            .map_err(|e| ControllerError::new("protocol", e.to_string()))?;

        if !response.ok {
            let error = response.error.unwrap_or(RemoteError {
                code: "unknown".into(),
                message: "emulator returned an unspecified error".into(),
            });
            return Err(ControllerError::new(error.code, error.message));
        }

        Ok(response.result)
    }

    fn unit(&self, method: &str, args: Value) -> CncResult<()> {
        self.call(method, args).map(|_| ())
    }
}

impl LinuxCncApi for LinuxCncApiClient {
    fn status(&self) -> CncResult<MachineStatus> {
        let value = self.call("status", json!({}))?;
        serde_json::from_value(value)
            .map_err(|e| ControllerError::new("protocol", e.to_string()))
    }

    fn power_on(&self) -> CncResult<()> {
        self.unit("power_on", json!({}))
    }

    fn power_off(&self) -> CncResult<()> {
        self.unit("power_off", json!({}))
    }

    fn estop(&self) -> CncResult<()> {
        self.unit("estop", json!({}))
    }

    fn home(&self, axis: Option<Axis>) -> CncResult<()> {
        self.unit("home", json!({ "axis": axis }))
    }

    fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        self.unit("select_coordinate_system", json!({ "system": system }))
    }

    fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        self.unit("set_work_zero", json!({ "system": system }))
    }

    fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        self.unit("jog_increment", json!({ "request": request }))
    }

    fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()> {
        self.unit(
            "jog_continuous",
            json!({ "axis": axis, "direction": direction, "feed_rate": feed_rate }),
        )
    }

    fn mdi(&self, command: &str) -> CncResult<()> {
        self.unit("mdi", json!({ "command": command }))
    }

    fn load_program(&self, path: &Path) -> CncResult<()> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| ControllerError::new("program_read", e.to_string()))?;
        let name = path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("program.ngc");
        self.unit("load_program", json!({ "name": name, "content": content }))
    }

    fn program_run(&self, start_line: u32) -> CncResult<()> {
        self.unit("program_run", json!({ "start_line": start_line }))
    }

    fn program_pause(&self) -> CncResult<()> {
        self.unit("program_pause", json!({}))
    }

    fn program_resume(&self) -> CncResult<()> {
        self.unit("program_resume", json!({}))
    }

    fn program_abort(&self) -> CncResult<()> {
        self.unit("program_abort", json!({}))
    }

    fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        self.unit("set_spindle", json!({ "command": command }))
    }

    fn clear_error(&self) -> CncResult<()> {
        self.unit("clear_error", json!({}))
    }
}
