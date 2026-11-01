//! Shared asynchronous CNC control API and coarse controller-state classification.

use crate::types::{
    Axis, CncEvent, CoordinateSystem, JogRequest, MachineConfiguration, MachineState,
    MachineStatus, OverrideKind, ProgramSource, SpindleCommand, SpindleMode,
};
use crate::{discovery, CncResult, ControllerError};
use std::path::Path;

/// Converts detailed controller telemetry into the coarse UI state.
pub fn machine_state(status: &MachineStatus) -> MachineState {
    use crate::types::{InterpreterState, TaskState};
    if !status.connected {
        MachineState::Disconnected
    } else if status.task_state == TaskState::Estop
        || status
            .message
            .as_deref()
            .is_some_and(|message| message.starts_with("ALARM:"))
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

/// Shared async control API implemented by CNC backend adapters.
#[async_trait::async_trait]
pub trait CncApi: Send + Sync {
    /// Returns a nonblocking reference to adapter-specific behavior for backend integrations.
    fn klipper_adapter(&self) -> Option<&crate::adapters::klipper::KlipperAdapter> {
        None
    }
    /// Returns the latest complete controller state.
    async fn status(&self) -> CncResult<MachineStatus>;
    /// Returns cached discovered devices.
    fn discovered_devices(&self) -> Vec<discovery::DiscoveredDevice> {
        Vec::new()
    }
    /// Starts a streaming device discovery pass.
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
    /// Collects all devices yielded by the discovery stream.
    async fn rediscover_devices(&self) -> CncResult<Vec<discovery::DiscoveredDevice>> {
        let mut receiver = self.rediscover_devices_stream(Vec::new()).await?;
        let mut devices = Vec::new();
        while let Some(device) = receiver.recv().await {
            devices.push(device);
        }
        Ok(devices)
    }
    /// Stable id of the selected endpoint.
    fn connected_device_id(&self) -> Option<String> {
        None
    }
    /// Returns last status without network activity.
    fn last_status(&self) -> Option<MachineStatus> {
        None
    }
    /// Subscribes to status changes.
    fn subscribe_status(&self) -> Option<tokio::sync::watch::Receiver<MachineStatus>> {
        None
    }
    /// Subscribes to backend events.
    fn subscribe_events(&self) -> Option<tokio::sync::broadcast::Receiver<CncEvent>> {
        None
    }
    /// Loads a path or inline program where supported.
    async fn load_program_source(&self, source: ProgramSource) -> CncResult<()> {
        match source {
            ProgramSource::Path(path) => self.load_program(&path).await,
            ProgramSource::Inline(_) => Err(ControllerError::new(
                "unsupported",
                "inline program loading is not supported by this adapter",
            )),
        }
    }
    /// Selects a runtime laser mode when supported.
    async fn set_laser_mode(&self, mode: SpindleMode) -> CncResult<()> {
        let _ = mode;
        Err(ControllerError::new(
            "unsupported",
            "runtime laser mode selection is not supported by this adapter",
        ))
    }
    /// Sets a supported override percentage.
    async fn set_override(&self, kind: OverrideKind, value: f64) -> CncResult<()> {
        let _ = (kind, value);
        Err(ControllerError::new(
            "unsupported",
            "overrides are not supported by this adapter",
        ))
    }
    /// Returns controller configuration where available.
    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        Err(ControllerError::new(
            "unsupported",
            "machine configuration is not available for this adapter",
        ))
    }
    /// Enables controller outputs where available.
    async fn power_on(&self) -> CncResult<()>;
    /// Disables controller outputs where available.
    async fn power_off(&self) -> CncResult<()>;
    /// Requests emergency stop.
    async fn estop(&self) -> CncResult<()>;
    /// Homes all axes or the specified axis.
    async fn home(&self, axis: Option<Axis>) -> CncResult<()>;
    /// Selects the active work coordinate system.
    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()>;
    /// Zeros all work coordinates in one WCS.
    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()>;
    /// Zeros one work coordinate.
    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()>;
    /// Performs one incremental jog.
    async fn jog_increment(&self, request: JogRequest) -> CncResult<()>;
    /// Performs continuous single-axis jogging.
    async fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()>;
    /// Performs coordinated continuous XY jogging when supported.
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
    /// Cancels jogging when the controller supports it.
    async fn cancel_jog(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "jog cancellation is not supported by this adapter",
        ))
    }
    /// Stops controlled motion without emergency stop.
    async fn stop_motion(&self) -> CncResult<()> {
        self.cancel_jog().await
    }
    /// Executes one controller command with a cooperative cancellation token.
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
    /// Executes manual data input.
    async fn mdi(&self, command: &str) -> CncResult<()>;
    /// Loads a program from a host path.
    async fn load_program(&self, path: &Path) -> CncResult<()>;
    /// Starts a loaded program.
    async fn program_run(&self, start_line: u32) -> CncResult<()>;
    /// Pauses program execution.
    async fn program_pause(&self) -> CncResult<()>;
    /// Resumes program execution.
    async fn program_resume(&self) -> CncResult<()>;
    /// Cancels program execution.
    async fn program_abort(&self) -> CncResult<()>;
    /// Controls spindle or laser output.
    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()>;
    /// Clears a controller alarm or shutdown.
    async fn clear_error(&self) -> CncResult<()>;
}
