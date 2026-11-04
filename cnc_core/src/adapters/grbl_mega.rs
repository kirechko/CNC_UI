//! Explicit GRBL-Mega adapter name for the stock GRBL 1.1 serial protocol.
//!
//! GRBL-Mega's compatible builds use the GRBL line/status protocol. Firmware
//! forks with a changed protocol must not be selected under this alias.

use crate::adapters::grbl::SerialProtocol;
use crate::{CncApi, CncResult};
use std::path::Path;

/// GRBL-Mega controller wrapper over the existing GRBL 1.1 serial adapter.
pub struct GrblMegaAdapter {
    /// Shared GRBL-compatible serial implementation.
    inner: crate::adapters::grbl::GrblAdapter,
}

impl GrblMegaAdapter {
    /// Opens the GRBL-Mega serial port; baud rate must match firmware settings.
    pub fn connect(port: &str, baud_rate: u32) -> CncResult<Self> {
        Ok(Self {
            inner: crate::adapters::grbl::GrblAdapter::connect_with_protocol(
                port,
                baud_rate,
                SerialProtocol::Grbl,
                None,
            )?,
        })
    }
}

#[async_trait::async_trait]
impl CncApi for GrblMegaAdapter {
    /// Returns GRBL realtime status through the shared serial adapter.
    async fn status(&self) -> CncResult<crate::MachineStatus> {
        self.inner.status().await
    }
    /// Reports the endpoint with the GRBL protocol identity used by discovery.
    fn discovered_devices(&self) -> Vec<crate::discovery::DiscoveredDevice> {
        self.inner.discovered_devices()
    }
    /// Exposes the selected GRBL serial device id.
    fn connected_device_id(&self) -> Option<String> {
        self.inner.connected_device_id()
    }
    /// Reads GRBL settings for travel and spindle configuration.
    async fn machine_configuration(&self) -> CncResult<crate::MachineConfiguration> {
        self.inner.machine_configuration().await
    }
    /// Unlocks an alarm through the GRBL `$X` command.
    async fn power_on(&self) -> CncResult<()> {
        self.inner.power_on().await
    }
    /// GRBL-Mega serial does not expose a safe software power-off.
    async fn power_off(&self) -> CncResult<()> {
        self.inner.power_off().await
    }
    /// Serial commands are not an emergency stop.
    async fn estop(&self) -> CncResult<()> {
        self.inner.estop().await
    }
    /// Homes all axes as supported by the underlying GRBL firmware.
    async fn home(&self, axis: Option<crate::Axis>) -> CncResult<()> {
        self.inner.home(axis).await
    }
    /// Selects an active coordinate system.
    async fn select_coordinate_system(&self, system: crate::CoordinateSystem) -> CncResult<()> {
        self.inner.select_coordinate_system(system).await
    }
    /// Sets a work coordinate origin.
    async fn set_work_zero(&self, system: crate::CoordinateSystem) -> CncResult<()> {
        self.inner.set_work_zero(system).await
    }
    /// Sets one work coordinate origin.
    async fn set_axis_work_zero(&self, axis: crate::Axis) -> CncResult<()> {
        self.inner.set_axis_work_zero(axis).await
    }
    /// Performs one incremental jog.
    async fn jog_increment(&self, request: crate::JogRequest) -> CncResult<()> {
        self.inner.jog_increment(request).await
    }
    /// Performs continuous axis jogging.
    async fn jog_continuous(
        &self,
        axis: crate::Axis,
        direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        self.inner.jog_continuous(axis, direction, feed_rate).await
    }
    /// Performs coordinated XY jogging.
    async fn jog_continuous_xy(&self, x: i8, y: i8, feed: f64) -> CncResult<()> {
        self.inner.jog_continuous_xy(x, y, feed).await
    }
    /// Cancels active jog motion.
    async fn cancel_jog(&self) -> CncResult<()> {
        self.inner.cancel_jog().await
    }
    /// Executes one G-code command.
    async fn mdi(&self, command: &str) -> CncResult<()> {
        self.inner.mdi(command).await
    }
    /// Loads a host G-code file.
    async fn load_program(&self, path: &Path) -> CncResult<()> {
        self.inner.load_program(path).await
    }
    /// Streams a loaded program.
    async fn program_run(&self, start: u32) -> CncResult<()> {
        self.inner.program_run(start).await
    }
    /// Pauses program execution.
    async fn program_pause(&self) -> CncResult<()> {
        self.inner.program_pause().await
    }
    /// Resumes program execution.
    async fn program_resume(&self) -> CncResult<()> {
        self.inner.program_resume().await
    }
    /// Aborts program execution.
    async fn program_abort(&self) -> CncResult<()> {
        self.inner.program_abort().await
    }
    /// Controls spindle direction and speed.
    async fn set_spindle(&self, command: crate::SpindleCommand) -> CncResult<()> {
        self.inner.set_spindle(command).await
    }
    /// Clears the alarm state.
    async fn clear_error(&self) -> CncResult<()> {
        self.inner.clear_error().await
    }
    /// Executes a command with cancellation support.
    async fn execute_command(
        &self,
        command: &str,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> CncResult<String> {
        self.inner.execute_command(command, cancellation).await
    }
    /// Sends a coordinated stop operation.
    async fn stop_motion(&self) -> CncResult<()> {
        self.inner.stop_motion().await
    }
}
