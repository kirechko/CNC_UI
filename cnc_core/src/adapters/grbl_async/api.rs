//! The `CncApi` surface of [`GrblAdapter`]: discovery identity, status
//! polling (which also drives streaming and continuous jog) and the machine
//! configuration built from `$` settings.
//!
//! Because Rust permits only one `impl Trait for Type` block per type, the
//! complete `CncApi` surface lives in this single file; the shared logic
//! (continuous jog, program streaming, guards, parsing) is factored into
//! `pub(super)` helpers that live in the sibling modules.

use super::{wait_for_command_cancellation, ContinuousJog, GrblAdapter, SerialProtocol};
use crate::discovery::{DiscoveredDevice, DiscoveryTransport};
use crate::{
    Axis, CncApi, CncResult, ControllerError, CoordinateSystem, JogRequest, MachineConfiguration,
    MachineStatus, SpindleCommand, SpindleDirection,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

mod configuration;
mod status;

#[async_trait::async_trait]
impl CncApi for GrblAdapter {
    /// The serial family (GRBL or FluidNC — task: FluidNC over COM port) is
    /// reached over one port, so exactly one "device" exists: the endpoint the
    /// adapter was opened on. The id prefix, protocol label and machine name
    /// mirror the discovery scanner's record so the connected row highlights
    /// in the machines dialog.
    fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        let is_grbl_mega = self.protocol == SerialProtocol::Grbl
            && self
                .machine_name
                .as_deref()
                .is_some_and(|name| name.to_ascii_lowercase().contains("grbl-mega"));
        vec![DiscoveredDevice {
            id: if is_grbl_mega {
                format!("grbl_mega:serial:{}", self.serial_endpoint)
            } else {
                format!(
                    "{}:serial:{}",
                    self.protocol.id_prefix(),
                    self.serial_endpoint
                )
            },
            protocol: if is_grbl_mega {
                "GRBL-Mega".into()
            } else {
                self.protocol.label().into()
            },
            machine_name: self.machine_name.clone(),
            version: None,
            transport: DiscoveryTransport::Serial,
            // Full device node path: the scanner stores the canonical path in
            // `endpoint`, and the machines dialog shows it as the address.
            endpoint: self.serial_endpoint.clone(),
        }]
    }

    fn connected_device_id(&self) -> Option<String> {
        self.discovered_devices()
            .first()
            .map(|device| device.id.clone())
    }

    /// Queries one realtime status report and advances the active scheduler.
    async fn status(&self) -> CncResult<MachineStatus> {
        self.api_status().await
    }

    /// Reads the controller settings into the public machine configuration.
    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        self.api_machine_configuration().await
    }

    async fn power_on(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::send_command(&mut session, "$X").await
    }

    async fn power_off(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "GRBL has no safe software power-off command; use the machine hardware controls",
        ))
    }

    async fn estop(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "GRBL serial commands are not an emergency stop; use the machine hardware E-Stop",
        ))
    }

    async fn home(&self, axis: Option<Axis>) -> CncResult<()> {
        if axis.is_some() {
            return Err(ControllerError::new(
                "unsupported",
                "GRBL $H homes all configured axes; individual-axis homing is unsupported",
            ));
        }
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        // GRBL pauses normal serial reads while the homing cycle runs and only
        // acknowledges `$H` once every axis is found, which routinely exceeds
        // the regular 3-second ACK window on real machines.
        Self::send_command_with_timeout(&mut session, "$H", super::PROGRAM_ACK_TIMEOUT).await
    }

    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        Self::ensure_supported_coordinate_system(system)?;
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::send_command(&mut session, system.label()).await?;
        session.active_coordinate_system = system;
        Ok(())
    }

    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        Self::ensure_supported_coordinate_system(system)?;
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        let command = format!("G10 L20 P{} X0 Y0 Z0", Self::coordinate_number(system));
        Self::send_command(&mut session, &command).await?;
        // G10 L20 Pn updates the offset of the given system but does not select it.
        // Only select_coordinate_system() changes the active system.
        session.work_offsets.remove(&system);
        Ok(())
    }

    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        Self::ensure_supported_axis(self.protocol, axis)?;
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::ensure_supported_coordinate_system(session.active_coordinate_system)?;
        let command = format!(
            "G10 L20 P{} {}0",
            Self::coordinate_number(session.active_coordinate_system),
            Self::axis_letter(axis)
        );
        Self::send_command(&mut session, &command).await?;
        let active_coordinate_system = session.active_coordinate_system;
        session.work_offsets.remove(&active_coordinate_system);
        Ok(())
    }

    /// One incremental step along a single axis, sent as a blocking `$J=` line.
    async fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        Self::ensure_supported_axis(self.protocol, request.axis)?;

        if ![-1, 1].contains(&request.direction)
            || !request.distance.is_finite()
            || request.distance <= 0.0
        {
            return Err(ControllerError::new("bad_jog", "invalid GRBL jog request"));
        }
        let feed_rate = request.feed_rate.unwrap_or(100.0);
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }

        let value = request.distance * f64::from(request.direction);
        let command = Self::format_axis_jog_command(request.axis, value, feed_rate)?;
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::send_command(&mut session, &command).await
    }

    /// Starts (or, with `direction == 0`, cancels) a continuous jog along one
    /// axis. The actual chunk scheduling is delegated to
    /// [`GrblAdapter::run_continuous_jog`] so the axis and XY variants share
    /// one implementation.
    async fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()> {
        if ![-1, 0, 1].contains(&direction) {
            return Err(ControllerError::new(
                "bad_jog",
                "invalid continuous jog direction",
            ));
        }
        if direction == 0 {
            return self.cancel_jog().await;
        }
        Self::ensure_supported_axis(self.protocol, axis)?;
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }

        self.run_continuous_jog(ContinuousJog {
            x_direction: direction,
            y_direction: 0,
            axis: Some(axis),
            feed_rate,
        })
        .await
    }

    /// Continuous diagonal jog in the XY plane; direction/precision validation
    /// happens in [`GrblAdapter::validate_continuous_xy_jog`] and the chunk
    /// scheduling is shared with [`Self::jog_continuous`].
    async fn jog_continuous_xy(
        &self,
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        Self::validate_continuous_xy_jog(x_direction, y_direction, feed_rate)?;

        if x_direction == 0 && y_direction == 0 {
            return self.cancel_jog().await;
        }

        self.run_continuous_jog(ContinuousJog {
            x_direction,
            y_direction,
            axis: None,
            feed_rate,
        })
        .await
    }

    /// Clears any pending continuous jog and sends the jog-cancel byte (0x85).
    async fn cancel_jog(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        session.continuous_jog = None;
        session.jog_chunk_active = false;
        Self::send_realtime(&mut session, 0x85).await
    }

    /// Stops all motion: jog-cancel when a jog was active, otherwise a GRBL
    /// feed hold so the program state and the serial protocol stay intact.
    async fn stop_motion(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        let jog_active = session.continuous_jog.is_some() || session.jog_chunk_active;
        session.continuous_jog = None;
        session.jog_chunk_active = false;

        if jog_active {
            Self::send_realtime(&mut session, 0x85).await
        } else {
            // GRBL feed hold stops regular motion in a controlled way,
            // without resetting the connection or performing a hardware E-Stop.
            Self::send_realtime(&mut session, b'!').await
        }
    }

    /// Runs one console/MDI command with cancellation support. `$$` is special
    /// cased: its multi-line reply is formatted as `$N=value` lines; a
    /// cancellation during the exchange triggers a soft reset and poisons the
    /// protocol because the reply may have been cut in half.
    async fn execute_command(
        &self,
        command: &str,
        cancellation: Arc<AtomicBool>,
    ) -> CncResult<String> {
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;

        if cancellation.load(Ordering::Acquire) {
            return Err(ControllerError::new(
                "command_cancelled",
                "Command cancelled.",
            ));
        }

        if command.trim() == "$$" {
            let result = tokio::select! {
                result = Self::query_settings(&mut session) => result,
                _ = wait_for_command_cancellation(cancellation.clone()) => {
                    Self::reset_after_command_cancellation(&mut session).await?;
                    return Err(ControllerError::new(
                        "command_cancelled",
                        "Command cancelled by a GRBL reset; reconnect to the controller.",
                    ));
                }
            };
            let mut settings = result?.into_iter().collect::<Vec<_>>();
            settings.sort_unstable_by_key(|(number, _)| *number);
            return Ok(settings
                .into_iter()
                .map(|(number, value)| format!("${number}={value}"))
                .collect::<Vec<_>>()
                .join("\\n"));
        }

        let result = tokio::select! {
            result = Self::send_command(&mut session, command) => result,
            _ = wait_for_command_cancellation(cancellation) => {
                Self::reset_after_command_cancellation(&mut session).await?;
                return Err(ControllerError::new(
                    "command_cancelled",
                    "Command cancelled by a GRBL reset; reconnect to the controller.",
                ));
            }
        };
        result?;
        Ok("Command accepted by the controller.".into())
    }

    /// Manual Data Input: fire one G-code line without awaiting a reply.
    async fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.session.lock().await;
        // Remember the modal motion group of MDI moves for the telemetry
        // indication (task T12); only plain motion words qualify.
        if let Some(mode) = Self::motion_mode_of(command) {
            session.motion_mode = Some(mode.to_string());
        }
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::send_command(&mut session, command).await
    }

    /// Reads a whole G-code file into the session before any motion state is
    /// touched, so a bad file never leaves the adapter half-loaded. The file
    /// I/O, line validation and toolpath parsing live in
    /// [`GrblAdapter::load_program_file`] (`program.rs`).
    async fn load_program(&self, path: &Path) -> CncResult<()> {
        self.load_program_file(path).await
    }

    /// Begins stop-and-wait streaming from `start_line` once GRBL confirms it
    /// is Idle.
    async fn program_run(&self, start_line: u32) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::ensure_program_idle(
            session.program_streaming,
            session.continuous_jog.is_some() || session.jog_chunk_active,
        )?;
        let status = Self::query_status(&mut session).await?;
        Self::ensure_controller_idle(&status)?;

        let start = start_line as usize;
        if session.program.is_empty() || start >= session.program.len() {
            return Err(ControllerError::new(
                "bad_program",
                "no program loaded or start line is outside the loaded GRBL program",
            ));
        }

        session.current_line = start_line;
        session.program_streaming = true;
        // Kick the non-blocking scheduler: this writes the very first line
        // (the controller was just proven Idle), later lines advance on the
        // 10 Hz status polls as planner acknowledgements arrive.
        Self::pump_program_stream(&mut session, true).await
    }

    /// Feed hold (realtime `!`): GRBL keeps the program position.
    async fn program_pause(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::send_realtime(&mut session, b'!').await
    }

    /// Cycle start/resume (realtime `~`) releases a feed hold.
    async fn program_resume(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::send_realtime(&mut session, b'~').await
    }

    /// Soft reset (0x18): stops streaming, cancels jog and rewinds the cursor.
    async fn program_abort(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        session.program_streaming = false;
        // A soft reset flushes the planner; whatever acknowledgement the last
        // written line might still have produced will never arrive, so drop
        // the pending bookkeeping together with the streaming state.
        session.pending_ack = None;
        session.continuous_jog = None;
        session.jog_chunk_active = false;
        Self::send_realtime(&mut session, 0x18).await?;
        session.current_line = 0;
        // Soft reset rewinds the machine; the remembered traversed path is
        // invalid from now on so the next run highlights from the start.
        session.last_progress = None;
        Ok(())
    }

    /// Applies a spindle RPM or native laser S setpoint, respecting controller settings.
    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        if !command.rpm.is_finite() || command.rpm < 0.0 {
            return Err(ControllerError::new(
                "bad_spindle",
                "spindle speed or laser power must be finite and non-negative",
            ));
        }

        let command = Self::normalize_spindle_command(command);
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::validate_spindle_command(command, session.min_spindle_rpm, session.max_spindle_rpm)?;
        if session.spindle_mode != crate::SpindleMode::Standard
            && command.direction == SpindleDirection::CounterClockwise
        {
            return Err(ControllerError::new(
                "laser_direction",
                "laser output has no reverse direction",
            ));
        }
        let gcode = match command.direction {
            SpindleDirection::Stopped => "M5".to_owned(),
            SpindleDirection::Clockwise
                if session.spindle_mode == crate::SpindleMode::LaserDynamic =>
            {
                format!("M4 S{}", command.rpm)
            }
            SpindleDirection::Clockwise => format!("M3 S{}", command.rpm),
            SpindleDirection::CounterClockwise => format!("M4 S{}", command.rpm),
        };
        Self::send_command(&mut session, &gcode).await?;
        session.spindle_command = command;
        Ok(())
    }

    /// Confirms that an explicitly detected stock GRBL laser supports the requested behavior.
    async fn set_laser_mode(&self, mode: crate::SpindleMode) -> CncResult<()> {
        if mode == crate::SpindleMode::Standard {
            return Err(ControllerError::new(
                "bad_laser_mode",
                "standard spindle mode is not a selectable laser mode",
            ));
        }
        let mut session = self.session.lock().await;
        if session.protocol != SerialProtocol::Grbl
            || session.spindle_mode == crate::SpindleMode::Standard
        {
            return Err(ControllerError::new(
                "unsupported",
                "GRBL laser mode is unavailable; verify controller setting $32 equals 1",
            ));
        }
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        session.spindle_mode = mode;
        Ok(())
    }

    /// Clears an alarm state with `$X` (reset error).
    async fn clear_error(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::send_command(&mut session, "$X").await
    }
}
