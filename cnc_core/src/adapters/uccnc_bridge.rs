//! TCP JSON-lines client for an explicitly installed UCCNC companion plugin.
//!
//! UCCNC is a Windows application and has no universal native network API.
//! This module defines a small opt-in bridge contract; only a separately
//! installed plugin implementing this exact contract can be controlled.

use crate::{
    Axis, CncApi, CncResult, ControllerError, CoordinateSystem, JogRequest, MachineConfiguration,
    MachineStatus, SpindleCommand,
};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::Mutex,
    time::timeout,
};

/// Connection settings for a local or network UCCNC companion bridge.
#[derive(Debug, Clone)]
pub struct UccncBridgeConfig {
    /// Bridge TCP address in `host:port` form.
    pub endpoint: String,
    /// Timeout for TCP connection and one JSON-lines response.
    pub request_timeout: Duration,
}

impl UccncBridgeConfig {
    /// Creates a bridge configuration with a conservative timeout.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            request_timeout: Duration::from_secs(5),
        }
    }
}

/// JSON request envelope sent to the companion plugin.
#[derive(Debug, Serialize)]
struct Request<'a> {
    /// Monotonic request id for response correlation.
    id: u64,
    /// Operation name defined by the bridge contract.
    operation: &'a str,
    /// Operation-specific JSON arguments.
    payload: serde_json::Value,
}

/// JSON response envelope returned by the companion plugin.
#[derive(Debug, Deserialize)]
struct Response {
    /// Correlation id copied from the request.
    id: u64,
    /// Whether the operation completed successfully.
    ok: bool,
    /// Optional result object returned by the bridge.
    #[serde(default)]
    result: Option<serde_json::Value>,
    /// Optional controller or plugin error message.
    #[serde(default)]
    error: Option<String>,
}

/// Adapter for a UCCNC bridge; not a direct UCCNC controller implementation.
pub struct UccncBridgeAdapter {
    /// Endpoint and operation timeout settings.
    config: UccncBridgeConfig,
    /// Serializes RPC exchanges and owns the request counter.
    session: Mutex<Session>,
}

/// Mutable RPC sequencing state.
struct Session {
    /// Identifier assigned to the next request.
    next_id: u64,
}

impl UccncBridgeAdapter {
    /// Validates and stores an explicit bridge endpoint without connecting eagerly.
    pub fn connect(config: UccncBridgeConfig) -> CncResult<Self> {
        if config.endpoint.trim().is_empty() {
            return Err(ControllerError::new(
                "connect",
                "UCCNC bridge endpoint is empty",
            ));
        }
        Ok(Self {
            config,
            session: Mutex::new(Session { next_id: 1 }),
        })
    }

    /// Executes one bounded bridge RPC and validates the response correlation identifier.
    async fn call(
        &self,
        operation: &str,
        payload: serde_json::Value,
    ) -> CncResult<serde_json::Value> {
        let mut session = self.session.lock().await;
        let id = session.next_id;
        session.next_id = session.next_id.wrapping_add(1).max(1);
        let request = serde_json::to_vec(&Request {
            id,
            operation,
            payload,
        })
        .map_err(|error| ControllerError::new("protocol", error.to_string()))?;
        let exchange = async {
            let stream = TcpStream::connect(&self.config.endpoint)
                .await
                .map_err(|error| ControllerError::new("transport", error.to_string()))?;
            let (read, mut write) = stream.into_split();
            write
                .write_all(&request)
                .await
                .map_err(|error| ControllerError::new("transport", error.to_string()))?;
            write
                .write_all(b"\n")
                .await
                .map_err(|error| ControllerError::new("transport", error.to_string()))?;
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            if reader
                .read_line(&mut line)
                .await
                .map_err(|error| ControllerError::new("transport", error.to_string()))?
                == 0
            {
                return Err(ControllerError::new(
                    "disconnected",
                    "UCCNC bridge closed without a response",
                ));
            }
            let response: Response = serde_json::from_str(&line)
                .map_err(|error| ControllerError::new("protocol", error.to_string()))?;
            if response.id != id {
                return Err(ControllerError::new(
                    "protocol",
                    "UCCNC bridge response id does not match request",
                ));
            }
            if !response.ok {
                return Err(ControllerError::new(
                    "controller",
                    response
                        .error
                        .unwrap_or_else(|| "UCCNC bridge rejected the operation".into()),
                ));
            }
            Ok(response.result.unwrap_or(serde_json::Value::Null))
        };
        timeout(self.config.request_timeout, exchange)
            .await
            .map_err(|_| ControllerError::new("timeout", "UCCNC bridge request timed out"))?
    }
}

#[async_trait::async_trait]
impl CncApi for UccncBridgeAdapter {
    /// Requests a normalized machine status snapshot from the bridge.
    async fn status(&self) -> CncResult<MachineStatus> {
        serde_json::from_value(self.call("status", serde_json::Value::Null).await?).map_err(
            |error| ControllerError::new("protocol", format!("invalid UCCNC status: {error}")),
        )
    }
    /// Requests supported axes and spindle limits from the bridge.
    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        serde_json::from_value(
            self.call("machine_configuration", serde_json::Value::Null)
                .await?,
        )
        .map_err(|error| {
            ControllerError::new("protocol", format!("invalid UCCNC configuration: {error}"))
        })
    }
    /// Enables outputs through the configured plugin operation.
    async fn power_on(&self) -> CncResult<()> {
        self.call("power_on", serde_json::Value::Null)
            .await
            .map(|_| ())
    }
    /// Disables outputs through the configured plugin operation.
    async fn power_off(&self) -> CncResult<()> {
        self.call("power_off", serde_json::Value::Null)
            .await
            .map(|_| ())
    }
    /// Requests the plugin emergency-stop operation; independent hardware E-stop remains required.
    async fn estop(&self) -> CncResult<()> {
        self.call("estop", serde_json::Value::Null)
            .await
            .map(|_| ())
    }
    /// Requests all-axis or selected-axis homing.
    async fn home(&self, axis: Option<Axis>) -> CncResult<()> {
        self.call("home", serde_json::json!({"axis": axis}))
            .await
            .map(|_| ())
    }
    /// Selects a work coordinate system.
    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        self.call(
            "select_coordinate_system",
            serde_json::json!({"system": system}),
        )
        .await
        .map(|_| ())
    }
    /// Sets all work coordinates of the selected system to zero.
    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        self.call("set_work_zero", serde_json::json!({"system": system}))
            .await
            .map(|_| ())
    }
    /// Sets one axis work coordinate to zero.
    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        self.call("set_axis_work_zero", serde_json::json!({"axis": axis}))
            .await
            .map(|_| ())
    }
    /// Requests a single incremental jog.
    async fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        self.call(
            "jog_increment",
            serde_json::to_value(request)
                .map_err(|error| ControllerError::new("protocol", error.to_string()))?,
        )
        .await
        .map(|_| ())
    }
    /// Requests a single-axis continuous jog.
    async fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()> {
        self.call(
            "jog_continuous",
            serde_json::json!({"axis": axis, "direction": direction, "feed_rate": feed_rate}),
        )
        .await
        .map(|_| ())
    }
    /// Requests a coordinated continuous XY jog.
    async fn jog_continuous_xy(
        &self,
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        self.call("jog_continuous_xy", serde_json::json!({"x_direction": x_direction, "y_direction": y_direction, "feed_rate": feed_rate})).await.map(|_| ())
    }
    /// Cancels active jog motion.
    async fn cancel_jog(&self) -> CncResult<()> {
        self.call("cancel_jog", serde_json::Value::Null)
            .await
            .map(|_| ())
    }
    /// Sends one manual command to the plugin.
    async fn mdi(&self, command: &str) -> CncResult<()> {
        self.call("mdi", serde_json::json!({"command": command}))
            .await
            .map(|_| ())
    }
    /// Asks the bridge to load a host-visible G-code path.
    async fn load_program(&self, path: &Path) -> CncResult<()> {
        self.call("load_program", serde_json::json!({"path": path}))
            .await
            .map(|_| ())
    }
    /// Starts program execution at the requested source line.
    async fn program_run(&self, start_line: u32) -> CncResult<()> {
        self.call("program_run", serde_json::json!({"start_line": start_line}))
            .await
            .map(|_| ())
    }
    /// Pauses the current program.
    async fn program_pause(&self) -> CncResult<()> {
        self.call("program_pause", serde_json::Value::Null)
            .await
            .map(|_| ())
    }
    /// Resumes the current program.
    async fn program_resume(&self) -> CncResult<()> {
        self.call("program_resume", serde_json::Value::Null)
            .await
            .map(|_| ())
    }
    /// Aborts the current program.
    async fn program_abort(&self) -> CncResult<()> {
        self.call("program_abort", serde_json::Value::Null)
            .await
            .map(|_| ())
    }
    /// Applies spindle direction and speed.
    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        self.call(
            "set_spindle",
            serde_json::to_value(command)
                .map_err(|error| ControllerError::new("protocol", error.to_string()))?,
        )
        .await
        .map(|_| ())
    }
    /// Clears the bridge-reported controller alarm.
    async fn clear_error(&self) -> CncResult<()> {
        self.call("clear_error", serde_json::Value::Null)
            .await
            .map(|_| ())
    }
}
