//! The shared asynchronous control API ([`CncApi`]) implemented by every CNC
//! backend adapter, plus the [`machine_state`] classifier that maps a detailed
//! [`MachineStatus`] snapshot onto the coarse UI state used in events.
//!
//! The trait lives in one file because Rust allows a single trait definition
//! and each adapter keeps a single `impl CncApi` block (see the adapter
//! modules); splitting the trait across files is not possible.

use crate::types::{
    Axis, CncEvent, CoordinateSystem, JogRequest, MachineConfiguration, MachineState,
    MachineStatus, OverrideKind, ProgramSource, SpindleCommand,
};
use crate::{discovery, CncResult, ControllerError};
use std::path::Path;

/// Return the coarse UI state represented by a controller status snapshot.
pub fn machine_state(status: &MachineStatus) -> MachineState {
    use crate::types::{InterpreterState, TaskState};
    if !status.connected {
        MachineState::Disconnected
    } else if status.task_state == TaskState::Estop
        || status
            .message
            .as_deref()
            .is_some_and(|m| m.starts_with("ALARM:"))
    {
        MachineState::Alarm
    } else if status.homing {
        MachineState::Home
    } else if status
        .motion_mode
        .as_deref()
        .is_some_and(|mode| mode.eq_ignore_ascii_case("jog"))
    {
        MachineState::Jog
    } else if status.interpreter_state == InterpreterState::Paused {
        MachineState::Hold
    } else if status.interpreter_state == InterpreterState::Reading {
        MachineState::Run
    } else if status.task_state == TaskState::On {
        MachineState::Idle
    } else {
        MachineState::Unknown
    }
}

/// Shared asynchronous control API implemented by CNC backends.
#[async_trait::async_trait]
pub trait CncApi: Send + Sync {
    /// Query the controller and return its most recent complete status.
    async fn status(&self) -> CncResult<MachineStatus>;

    /// Return endpoints found by automatic discovery, if this adapter supports it.
    fn discovered_devices(&self) -> Vec<discovery::DiscoveredDevice> {
        Vec::new()
    }

    /// Start a discovery pass and stream each result as it is found.
    async fn rediscover_devices_stream(
        &self,
        _priority_targets: Vec<String>,
    ) -> CncResult<tokio::sync::mpsc::Receiver<discovery::DiscoveredDevice>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);
        for device in self.discovered_devices() {
            if sender.send(device).await.is_err() {
                break;
            }
        }
        Ok(receiver)
    }

    /// Run a new device discovery pass, if supported by this adapter.
    async fn rediscover_devices(&self) -> CncResult<Vec<discovery::DiscoveredDevice>> {
        let mut receiver = self.rediscover_devices_stream(Vec::new()).await?;
        let mut devices = Vec::new();
        while let Some(device) = receiver.recv().await {
            devices.push(device);
        }
        Ok(devices)
    }

    /// Stable identifier of the endpoint currently used by this adapter, when known.
    fn connected_device_id(&self) -> Option<String> {
        None
    }

    /// Return a nonblocking copy of the most recently published status, when supported.
    fn last_status(&self) -> Option<MachineStatus> {
        None
    }

    /// Subscribe to status snapshot changes, when supported by the adapter.
    fn subscribe_status(&self) -> Option<tokio::sync::watch::Receiver<MachineStatus>> {
        None
    }

    /// Subscribe to adapter events, when supported by the adapter.
    fn subscribe_events(&self) -> Option<tokio::sync::broadcast::Receiver<CncEvent>> {
        None
    }

    /// Load a path or inline program. The default supports existing path-based adapters only.
    async fn load_program_source(&self, source: ProgramSource) -> CncResult<()> {
        match source {
            ProgramSource::Path(path) => self.load_program(&path).await,
            ProgramSource::Inline(_) => Err(ControllerError::new(
                "unsupported",
                "inline program loading is not supported by this adapter",
            )),
        }
    }

    /// Set a feed, spindle, or rapid override as a percentage.
    async fn set_override(&self, kind: OverrideKind, value: f64) -> CncResult<()> {
        let _ = (kind, value);
        Err(ControllerError::new(
            "unsupported",
            "overrides are not supported by this adapter",
        ))
    }

    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        Err(ControllerError::new(
            "unsupported",
            "machine configuration is not available for this adapter",
        ))
    }

    async fn power_on(&self) -> CncResult<()>;
    async fn power_off(&self) -> CncResult<()>;
    async fn estop(&self) -> CncResult<()>;
    async fn home(&self, axis: Option<Axis>) -> CncResult<()>;
    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()>;
    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()>;
    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()>;
    async fn jog_increment(&self, request: JogRequest) -> CncResult<()>;
    async fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()>;

    async fn jog_continuous_xy(
        &self,
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        let _ = (x_direction, y_direction, feed_rate);
        Err(ControllerError::new(
            "unsupported",
            "coordinated continuous XY jog is not supported by this adapter",
        ))
    }

    async fn cancel_jog(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "jog cancellation is not supported by this adapter",
        ))
    }

    /// Stop the current controlled motion without performing an emergency stop.
    async fn stop_motion(&self) -> CncResult<()> {
        self.cancel_jog().await
    }

    /// Execute a controller command and return its text response when one is available.
    /// Adapters may support G-code and adapter-specific commands here.
    async fn execute_command(
        &self,
        command: &str,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> CncResult<String> {
        if cancellation.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ControllerError::new(
                "command_cancelled",
                "Command cancelled.",
            ));
        }
        self.mdi(command).await?;
        if cancellation.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ControllerError::new(
                "command_cancelled",
                "Command cancelled.",
            ));
        }
        Ok("Command accepted by the controller.".into())
    }

    async fn mdi(&self, command: &str) -> CncResult<()>;
    async fn load_program(&self, path: &Path) -> CncResult<()>;
    async fn program_run(&self, start_line: u32) -> CncResult<()>;
    async fn program_pause(&self) -> CncResult<()>;
    async fn program_resume(&self) -> CncResult<()>;
    async fn program_abort(&self) -> CncResult<()>;
    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()>;
    async fn clear_error(&self) -> CncResult<()>;
}
