//! Smoothieboard adapter built on the shared GRBL-compatible serial transport.
//!
//! Smoothieware's console shares the common `ok`/G-code pattern but status and
//! settings differ by build. This adapter is an explicit GRBL-compatible mode;
//! firmware-specific capabilities not validated by this protocol are surfaced
//! as limitations in the status message and report.

use crate::adapters::serial_compat::SerialDialect;
use crate::{CncApi, CncResult, ControllerError};
use std::path::Path;

/// Smoothieboard/Smoothieware serial adapter backed by the Marlin-compatible console framing.
pub struct SmoothieAdapter {
    /// Underlying line-protocol adapter, selected as Marlin's plain G-code dialect.
    inner: crate::adapters::serial_compat::SerialCompatAdapter,
}

impl SmoothieAdapter {
    /// Opens a Smoothieware serial console at the requested baud rate.
    pub fn connect(port: &str, baud_rate: u32) -> CncResult<Self> {
        Ok(Self {
            inner: crate::adapters::serial_compat::SerialCompatAdapter::connect(
                port,
                baud_rate,
                SerialDialect::Marlin,
            )?,
        })
    }
}

#[async_trait::async_trait]
impl CncApi for SmoothieAdapter {
    /// Returns the latest serial status sample while marking telemetry as firmware-limited.
    async fn status(&self) -> CncResult<crate::MachineStatus> {
        let mut status = self.inner.status().await?;
        status.message = Some(
            "Smoothieboard serial console; extended telemetry/configuration not mapped".into(),
        );
        Ok(status)
    }
    /// Forwards discovery rows using the selected serial endpoint.
    fn discovered_devices(&self) -> Vec<crate::discovery::DiscoveredDevice> {
        self.inner
            .discovered_devices()
            .into_iter()
            .map(|mut device| {
                device.id = device.id.replace("marlin:", "smoothie:");
                device.protocol = "Smoothieboard".into();
                device
            })
            .collect()
    }
    /// Returns the Smoothieboard-specific stable endpoint id.
    fn connected_device_id(&self) -> Option<String> {
        self.discovered_devices()
            .first()
            .map(|device| device.id.clone())
    }
    /// Configuration needs a Smoothieware-specific version/settings parser.
    async fn machine_configuration(&self) -> CncResult<crate::MachineConfiguration> {
        Err(unsupported("machine configuration"))
    }
    /// Smoothieboard output-enable semantics are machine-configuration-dependent.
    async fn power_on(&self) -> CncResult<()> {
        Err(unsupported("software power-on"))
    }
    /// Smoothieboard power-off requires hardware-level controls.
    async fn power_off(&self) -> CncResult<()> {
        Err(unsupported("software power-off"))
    }
    /// Serial G-code is not an emergency-stop channel.
    async fn estop(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "use the machine hardware E-Stop",
        ))
    }
    /// Sends the requested homing command through the shared G-code interface.
    async fn home(&self, axis: Option<crate::Axis>) -> CncResult<()> {
        self.inner.home(axis).await
    }
    /// Selects one standard work coordinate system through the common adapter.
    async fn select_coordinate_system(&self, system: crate::CoordinateSystem) -> CncResult<()> {
        self.inner.select_coordinate_system(system).await
    }
    /// Sets the selected work coordinate zero using G10 L20.
    async fn set_work_zero(&self, system: crate::CoordinateSystem) -> CncResult<()> {
        self.inner.set_work_zero(system).await
    }
    /// Zeros an axis in the current work coordinate system.
    async fn set_axis_work_zero(&self, axis: crate::Axis) -> CncResult<()> {
        self.inner.set_axis_work_zero(axis).await
    }
    /// Sends one relative jog using standard G-code.
    async fn jog_increment(&self, request: crate::JogRequest) -> CncResult<()> {
        self.inner.jog_increment(request).await
    }
    /// Rejects continuous jog until its stop semantics are verified on Smoothieware.
    async fn jog_continuous(
        &self,
        axis: crate::Axis,
        direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        let _ = (axis, direction, feed_rate);
        Err(unsupported("continuous jog"))
    }
    /// Rejects diagonal jog pending firmware-specific safety validation.
    async fn jog_continuous_xy(&self, x: i8, y: i8, feed: f64) -> CncResult<()> {
        let _ = (x, y, feed);
        Err(unsupported("continuous XY jog"))
    }
    /// Refuses guessed realtime cancellation bytes.
    async fn cancel_jog(&self) -> CncResult<()> {
        Err(unsupported("jog cancellation"))
    }
    /// Forwards one G-code command.
    async fn mdi(&self, command: &str) -> CncResult<()> {
        self.inner.mdi(command).await
    }
    /// Loads a local G-code file through the underlying line adapter.
    async fn load_program(&self, path: &Path) -> CncResult<()> {
        self.inner.load_program(path).await
    }
    /// Streams the loaded program through acknowledged serial lines.
    async fn program_run(&self, start: u32) -> CncResult<()> {
        self.inner.program_run(start).await
    }
    /// Pauses using the underlying adapter's controlled feed-hold command.
    async fn program_pause(&self) -> CncResult<()> {
        self.inner.program_pause().await
    }
    /// Resume semantics are kept explicit pending validation on supported versions.
    async fn program_resume(&self) -> CncResult<()> {
        Err(unsupported("program resume"))
    }
    /// Aborts the active program using the underlying controlled stop behavior.
    async fn program_abort(&self) -> CncResult<()> {
        self.inner.program_abort().await
    }
    /// Forwards spindle commands using standard M-codes.
    async fn set_spindle(&self, command: crate::SpindleCommand) -> CncResult<()> {
        self.inner.set_spindle(command).await
    }
    /// Alarm clearing varies by Smoothieware build and configuration.
    async fn clear_error(&self) -> CncResult<()> {
        Err(unsupported("alarm clearing"))
    }
    /// Sends a command and checks cancellation before and after the serial transaction.
    async fn execute_command(
        &self,
        command: &str,
        cancellation: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> CncResult<String> {
        self.inner.execute_command(command, cancellation).await
    }
    /// The standard adapter API does not accept inline file transfer paths.
    async fn load_program_source(&self, source: crate::ProgramSource) -> CncResult<()> {
        match source {
            crate::ProgramSource::Path(path) => self.load_program(&path).await,
            crate::ProgramSource::Inline(_) => Err(unsupported("inline program loading")),
        }
    }
}

/// Creates a consistent unsupported-capability error for Smoothieware.
fn unsupported(feature: &str) -> ControllerError {
    ControllerError::new(
        "unsupported",
        format!(
            "Smoothieboard {feature} is unavailable until firmware-specific behavior is validated"
        ),
    )
}
