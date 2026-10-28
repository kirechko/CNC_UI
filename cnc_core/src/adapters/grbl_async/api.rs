//! The `CncApi` surface of [`GrblAdapter`]: discovery identity, status
//! polling (which also drives streaming and continuous jog) and the machine
//! configuration built from `$` settings.
//!
//! Because Rust permits only one `impl Trait for Type` block per type, the
//! complete `CncApi` surface lives in this single file; the shared logic
//! (continuous jog, program streaming, guards, parsing) is factored into
//! `pub(super)` helpers that live in the sibling modules.

use super::{ContinuousJog, GrblAdapter, wait_for_command_cancellation};
use crate::discovery::{DiscoveredDevice, DiscoveryTransport};
use crate::{
    Axis, AxisConfiguration, AxisUnits, CncApi, ControllerError, CncResult, CoordinateSystem,
    InterpreterState, JogRequest, MachineAxis, MachineConfiguration, MachineStatus,
    SpindleCommand, SpindleConfiguration, SpindleDirection, TaskMode, ToolpathProgress,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[async_trait::async_trait]
impl CncApi for GrblAdapter {
    /// GRBL is reached over a serial port, so exactly one "device" exists:
    /// the endpoint the adapter was opened on.
    fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        vec![DiscoveredDevice {
            id: format!("grbl:serial:{}", self.serial_endpoint),
            protocol: "GRBL".into(),
            machine_name: None,
            version: None,
            transport: DiscoveryTransport::Serial,
            endpoint: self.serial_endpoint.clone(),
        }]
    }

    fn connected_device_id(&self) -> Option<String> {
        Some(format!("grbl:serial:{}", self.serial_endpoint))
    }

    /// Queries one realtime status report and, as a side effect, runs the
    /// scheduler: streams the next program line when streaming is active, or
    /// replays the next continuous-jog chunk when the controller is Idle.
    async fn status(&self) -> CncResult<MachineStatus> {
        let mut session = self.session.lock().await;
        let response = Self::query_status(&mut session).await?;

        if session.program_streaming && Self::should_stream_program(&response) {
            Self::stream_next_program_line(&mut session).await?;
        }

        if Self::is_idle_status(&response) {
            session.jog_chunk_active = false;
            if !session.program_streaming {
                if let Some(jog) = session.continuous_jog {
                    if let Err(error) = Self::send_jog_chunk(&mut session, jog).await {
                        session.continuous_jog = None;
                        return Err(error);
                    }
                    session.jog_chunk_active = true;
                }
            }
        }

        let active_coordinate_system = session.active_coordinate_system;
        let mut status = Self::parse_status(
            &response,
            session.work_offsets.get(&active_coordinate_system).copied(),
            active_coordinate_system,
            session.spindle_command,
            session.program_name.as_deref(),
            session.current_line,
            session.program.len() as u32,
        )?;
        status.max_spindle_rpm = session.max_spindle_rpm.unwrap_or(0.0);
        // Progress is derived from the tool position on the parsed toolpath.
        status.progress = session
            .toolpath
            .locate(&status.position)
            .unwrap_or_else(ToolpathProgress::unavailable);
        if session.program_streaming && status.interpreter_state == InterpreterState::Idle {
            status.task_mode = TaskMode::Auto;
            status.interpreter_state = InterpreterState::Reading;
            status.motion_mode = Some("G-code".into());
        }

        if response.contains("|WCO:")
            || (response.contains("|MPos:") && response.contains("|WPos:"))
        {
            session.work_offsets.insert(
                active_coordinate_system,
                [
                    status.work_offset.x,
                    status.work_offset.y,
                    status.work_offset.z,
                ],
            );
        }

        Ok(status)
    }

    /// Reads `$$` settings and maps the GRBL $1xx/$3x/$13x numbering into
    /// per-axis travel/velocity/acceleration and spindle RPM limits.
    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let mut session = self.session.lock().await;
        let settings = Self::query_settings(&mut session).await?;
        let travel = |number: u16| settings.get(&number).copied().filter(|value| *value > 0.0);
        let spindle_rpm =
            |number: u16| settings.get(&number).copied().filter(|value| *value >= 0.0);
        let min_rpm = spindle_rpm(31);
        let max_rpm =
            spindle_rpm(30).filter(|maximum| min_rpm.is_none_or(|minimum| minimum <= *maximum));
        session.min_spindle_rpm = min_rpm;
        session.max_spindle_rpm = max_rpm;

        Ok(MachineConfiguration {
            axes: vec![
                AxisConfiguration {
                    axis: MachineAxis::X,
                    units: AxisUnits::Linear,
                    max_travel: travel(130),
                    max_velocity: travel(110),
                    acceleration: travel(120),
                    steps_per_unit: travel(100),
                },
                AxisConfiguration {
                    axis: MachineAxis::Y,
                    units: AxisUnits::Linear,
                    max_travel: travel(131),
                    max_velocity: travel(111),
                    acceleration: travel(121),
                    steps_per_unit: travel(101),
                },
                AxisConfiguration {
                    axis: MachineAxis::Z,
                    units: AxisUnits::Linear,
                    max_travel: travel(132),
                    max_velocity: travel(112),
                    acceleration: travel(122),
                    steps_per_unit: travel(102),
                },
            ],
            spindle: SpindleConfiguration { min_rpm, max_rpm },
            settings,
        })
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
        Self::send_command(&mut session, "$H").await
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
        let command = format!(
            "G10 L20 P{} X0 Y0 Z0",
            Self::coordinate_number(system)
        );
        Self::send_command(&mut session, &command).await?;
        // G10 L20 Pn updates the offset of the given system but does not select it.
        // Only select_coordinate_system() changes the active system.
        session.work_offsets.remove(&system);
        Ok(())
    }

    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        Self::ensure_supported_axis(axis)?;
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
        Self::ensure_supported_axis(request.axis)?;

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
        Self::ensure_supported_axis(axis)?;
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
        Self::stream_next_program_line(&mut session).await
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
        session.continuous_jog = None;
        session.jog_chunk_active = false;
        Self::send_realtime(&mut session, 0x18).await?;
        session.current_line = 0;
        Ok(())
    }

    /// Applies the spindle setpoint as M3/M4/M5, respecting the RPM limits
    /// previously read from the controller settings.
    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        if !command.rpm.is_finite() || command.rpm < 0.0 {
            return Err(ControllerError::new(
                "bad_spindle",
                "spindle RPM must be finite and non-negative",
            ));
        }

        let command = Self::normalize_spindle_command(command);
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::validate_spindle_command(command, session.min_spindle_rpm, session.max_spindle_rpm)?;
        let gcode = match command.direction {
            SpindleDirection::Stopped => "M5".to_owned(),
            SpindleDirection::Clockwise => format!("M3 S{}", command.rpm),
            SpindleDirection::CounterClockwise => format!("M4 S{}", command.rpm),
        };
        Self::send_command(&mut session, &gcode).await?;
        session.spindle_command = command;
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
