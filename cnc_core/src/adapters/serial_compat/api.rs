//! Shared `CncApi` implementation for the Marlin and TinyG serial dialects.

use super::{protocol, SerialCompatAdapter, SerialDialect};
use crate::discovery::{DiscoveredDevice, DiscoveryTransport};
use crate::{
    Axis, CncApi, CncResult, ControllerError, CoordinateSystem, JogRequest, MachineConfiguration,
    MachineStatus, SpindleCommand, SpindleDirection, SpindleMode,
};
use std::path::Path;

#[async_trait::async_trait]
impl CncApi for SerialCompatAdapter {
    /// Reports the opened serial endpoint with a stable firmware-prefixed identifier.
    fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        let (protocol_name, prefix) = match self.dialect {
            SerialDialect::Marlin => ("Marlin", "marlin"),
            SerialDialect::TinyG => ("TinyG", "tinyg"),
        };
        vec![DiscoveredDevice {
            id: format!("{prefix}:serial:{}", self.endpoint),
            protocol: protocol_name.into(),
            machine_name: None,
            version: None,
            transport: DiscoveryTransport::Serial,
            endpoint: self.endpoint.clone(),
        }]
    }

    /// Returns the stable id for the serial endpoint currently held by this adapter.
    fn connected_device_id(&self) -> Option<String> {
        self.discovered_devices()
            .first()
            .map(|device| device.id.clone())
    }

    /// Queries current position through Marlin M114 or requests TinyG JSON status.
    async fn status(&self) -> CncResult<MachineStatus> {
        let mut session = self.session.lock().await;
        let response = match session.dialect {
            SerialDialect::Marlin => protocol::exchange(&mut session, "M114").await?,
            SerialDialect::TinyG => protocol::exchange(&mut session, "{\"sr\":null}").await?,
        };
        let mut status = protocol::parse_status(self.dialect, &response)?;
        status.loaded_program = session.program_name.clone();
        status.current_line = session.current_line;
        status.total_lines = session.program.len() as u32;
        status.spindle = crate::SpindleStatus {
            commanded_rpm: session.spindle.rpm,
            actual_rpm: 0.0,
            direction: session.spindle.direction,
            enabled: session.spindle.direction != SpindleDirection::Stopped,
            mode: SpindleMode::Standard,
            laser_power: None,
        };
        Ok(status)
    }

    /// Firmware settings differ between versions and are not mapped to avoid guessed machine limits.
    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        Err(unsupported("machine configuration"))
    }
    /// No generic software power-on command is assumed.
    async fn power_on(&self) -> CncResult<()> {
        Err(unsupported("software power control"))
    }
    /// No generic software power-off command is assumed.
    async fn power_off(&self) -> CncResult<()> {
        Err(unsupported("software power control"))
    }
    /// Serial commands are not a safety-rated emergency stop.
    async fn estop(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "use the machine hardware E-Stop; serial commands are not an emergency stop",
        ))
    }

    /// Requests configured all-axis homing, rejecting per-axis assumptions.
    async fn home(&self, axis: Option<Axis>) -> CncResult<()> {
        if axis.is_some() {
            return Err(unsupported("individual-axis homing"));
        }
        self.mdi("G28").await
    }
    /// Selects a conventional work coordinate system.
    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        self.mdi(system.label()).await
    }
    /// Sets XYZ work zero using the standard G10 L20 command.
    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        self.mdi(&format!("G10 L20 P{} X0 Y0 Z0", coordinate_number(system)))
            .await
    }
    /// Sets one work-axis zero after querying the active coordinate system.
    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        let letter = axis_letter(axis);
        let system = self.status().await?.active_coordinate_system;
        self.mdi(&format!("G10 L20 P{} {letter}0", coordinate_number(system)))
            .await
    }

    /// Sends one validated incremental jog with explicit return to absolute mode.
    async fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        if ![-1, 1].contains(&request.direction)
            || !request.distance.is_finite()
            || request.distance <= 0.0
        {
            return Err(ControllerError::new(
                "bad_jog",
                "invalid incremental jog request",
            ));
        }
        let feed = request.feed_rate.unwrap_or(100.0);
        if !feed.is_finite() || feed <= 0.0 {
            return Err(ControllerError::new("bad_jog", "jog feed must be positive"));
        }
        self.mdi(&format!(
            "G91 G1 {}{:.4} F{feed:.1} G90",
            axis_letter(request.axis),
            request.distance * f64::from(request.direction)
        ))
        .await
    }
    /// Rejects continuous jog until firmware-specific stop behavior is validated.
    async fn jog_continuous(&self, _axis: Axis, _direction: i8, _feed_rate: f64) -> CncResult<()> {
        Err(unsupported("continuous jog"))
    }
    /// Rejects continuous XY jog pending firmware-specific safety validation.
    async fn jog_continuous_xy(
        &self,
        _x_direction: i8,
        _y_direction: i8,
        _feed_rate: f64,
    ) -> CncResult<()> {
        Err(unsupported("continuous XY jog"))
    }
    /// Does not send an unverified jog-cancel byte.
    async fn cancel_jog(&self) -> CncResult<()> {
        Err(unsupported("jog cancellation"))
    }

    /// Executes one G-code command and checks cancellation before and after the exchange.
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
        Ok("Command acknowledged by controller.".into())
    }

    /// Sends one command through the selected native serial framing.
    async fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.session.lock().await;
        protocol::exchange(&mut session, command).await.map(|_| ())
    }

    /// Loads local G-code lines and rejects controller-incompatible embedded newlines.
    async fn load_program(&self, path: &Path) -> CncResult<()> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        let lines = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with(';') && !line.starts_with('('))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let mut session = self.session.lock().await;
        session.program = lines;
        session.program_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        session.current_line = 0;
        Ok(())
    }

    /// Streams loaded lines one-by-one, retaining only controller-acknowledged progress.
    async fn program_run(&self, start_line: u32) -> CncResult<()> {
        let mut session = self.session.lock().await;
        let start = start_line as usize;
        if start >= session.program.len() {
            return Err(ControllerError::new(
                "bad_program",
                "start line is outside the loaded program",
            ));
        }
        for index in start..session.program.len() {
            let command = session.program[index].clone();
            protocol::exchange(&mut session, &command).await?;
            session.current_line = index as u32 + 1;
        }
        Ok(())
    }
    /// Issues a controlled pause command; it does not act as an emergency stop.
    async fn program_pause(&self) -> CncResult<()> {
        self.mdi("M0").await
    }
    /// Resume semantics are firmware/source dependent and are intentionally unsupported.
    async fn program_resume(&self) -> CncResult<()> {
        Err(unsupported("program resume"))
    }
    /// Issues a controlled pause as a conservative program-abort fallback.
    async fn program_abort(&self) -> CncResult<()> {
        self.mdi("M0").await
    }
    /// Sets spindle speed and direction with standard M3/M4/M5 G-code.
    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        if !command.rpm.is_finite() || command.rpm < 0.0 {
            return Err(ControllerError::new(
                "bad_spindle",
                "spindle RPM must be finite and non-negative",
            ));
        }
        let code = match command.direction {
            SpindleDirection::Stopped => "M5".to_owned(),
            SpindleDirection::Clockwise => format!("M3 S{:.0}", command.rpm),
            SpindleDirection::CounterClockwise => format!("M4 S{:.0}", command.rpm),
        };
        self.mdi(&code).await?;
        self.session.lock().await.spindle = command;
        Ok(())
    }
    /// Does not issue an unverified alarm-clear sequence.
    async fn clear_error(&self) -> CncResult<()> {
        Err(unsupported("alarm clearing"))
    }
}

/// Builds a consistent explicit unsupported-capability error.
fn unsupported(feature: &str) -> ControllerError {
    ControllerError::new(
        "unsupported",
        format!("{feature} is not implemented for this firmware adapter"),
    )
}
/// Maps conventional work coordinate systems to G10's P number.
fn coordinate_number(system: CoordinateSystem) -> u8 {
    match system {
        CoordinateSystem::G54 => 1,
        CoordinateSystem::G55 => 2,
        CoordinateSystem::G56 => 3,
        CoordinateSystem::G57 => 4,
        CoordinateSystem::G58 => 5,
        CoordinateSystem::G59 => 6,
        CoordinateSystem::G59_1 => 7,
        CoordinateSystem::G59_2 => 8,
        CoordinateSystem::G59_3 => 9,
    }
}
/// Converts each shared axis variant to its G-code letter.
fn axis_letter(axis: Axis) -> char {
    match axis {
        Axis::X => 'X',
        Axis::Y => 'Y',
        Axis::Z => 'Z',
        Axis::A => 'A',
        Axis::B => 'B',
        Axis::C => 'C',
    }
}
