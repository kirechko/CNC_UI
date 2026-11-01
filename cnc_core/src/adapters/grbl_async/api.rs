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
    Axis, AxisConfiguration, AxisUnits, CncApi, CncResult, ControllerError, CoordinateSystem,
    InterpreterState, JogRequest, MachineAxis, MachineConfiguration, MachineStatus, Position,
    SpindleCommand, SpindleConfiguration, SpindleDirection, TaskMode, ToolpathProgress,
};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[async_trait::async_trait]
impl CncApi for GrblAdapter {
    /// The serial family (GRBL or FluidNC — task: FluidNC over COM port) is
    /// reached over one port, so exactly one "device" exists: the endpoint the
    /// adapter was opened on. The id prefix, protocol label and machine name
    /// mirror the discovery scanner's record so the connected row highlights
    /// in the machines dialog.
    fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        vec![DiscoveredDevice {
            id: format!(
                "{}:serial:{}",
                self.protocol.id_prefix(),
                self.serial_endpoint
            ),
            protocol: self.protocol.label().into(),
            machine_name: self.machine_name.clone(),
            version: None,
            transport: DiscoveryTransport::Serial,
            // Full device node path: the scanner stores the canonical path in
            // `endpoint`, and the machines dialog shows it as the address.
            endpoint: self.serial_endpoint.clone(),
        }]
    }

    fn connected_device_id(&self) -> Option<String> {
        Some(format!(
            "{}:serial:{}",
            self.protocol.id_prefix(),
            self.serial_endpoint
        ))
    }

    /// Queries one realtime status report and, as a side effect, runs the
    /// scheduler: streams the next program line when streaming is active, or
    /// replays the next continuous-jog chunk when the controller is Idle.
    async fn status(&self) -> CncResult<MachineStatus> {
        let mut session = self.session.lock().await;
        let response = Self::query_status(&mut session).await?;

        // Streaming scheduler pass. The pump resolves any in-flight line
        // acknowledgement that `query_status` routed from the wire in ANY
        // machine state (a late `ok`/`error:`/`ALARM:` must never be lost),
        // but only writes the next line while the controller reports
        // Idle/Run/Check. Nothing here blocks: the old stop-and-wait send
        // inside status() froze the whole poll for up to the planner-ack
        // budget, which is what made the trajectory and coordinates advance
        // in multi-second jumps on GRBL.
        if session.program_streaming {
            let allow_write = Self::should_stream_program(&response);
            Self::pump_program_stream(&mut session, allow_write).await?;
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
            session.spindle_mode,
            session.program_name.as_deref(),
            session.current_line,
            session.program.len() as u32,
        )?;
        // Task T12: replace the generic "G-code" motion mode the parser emits
        // for the Run state with the precise modal group (G0/G1/G2/G3) the
        // adapter last acknowledged into the planner. GRBL's '?' report does
        // not carry it; the streamer/MDI remember what they sent.
        if status.motion_mode.as_deref() == Some("G-code") {
            if let Some(mode) = session.motion_mode.clone() {
                status.motion_mode = Some(mode);
            }
        }
        status.max_spindle_rpm = session.max_spindle_rpm.unwrap_or(0.0);
        // Task: toolpath display for GRBL. The toolpath is parsed from the
        // program text, i.e. it lives in the WORK frame of the active
        // coordinate system, while the parsed status carries the MACHINE
        // position. Locating the machine position on a work-frame path fails
        // whenever the work offset is non-zero, so the traversed part of the
        // path never highlighted. Subtract the work offset first, exactly
        // like the FluidNC WebSocket adapter does.
        let work_position = Position {
            x: status.position.x - status.work_offset.x,
            y: status.position.y - status.work_offset.y,
            z: status.position.z - status.work_offset.z,
            a: status.position.a - status.work_offset.a,
            b: status.position.b - status.work_offset.b,
            c: status.position.c - status.work_offset.c,
        };
        // GRBL acknowledges a line when it ENTERS the planner buffer, but the
        // answer for an arc block (G2/G3) is deferred until the WHOLE arc has
        // been converted into planner segments and accepted: GRBL segments
        // arcs internally (~$ARC_TOLERANCE per chord) and only drains its
        // 16-block planner while the machine executes it. On a long arc the
        // `ok` can therefore arrive at ~75-80% of the physical block, with
        // the tool already far inside the line. A plain `locate` would jump
        // the highlight onto later passes crossing the same coordinates, so
        // the search stays forward-only and monotonic (floor = last reported
        // distance), but its upper bound must include the in-flight line:
        // the line written to GRBL whose `ok` has not been observed yet is
        // exactly the line the tool is cutting. Excluding it froze the arc
        // highlight until the delayed acknowledgement arrived (the 80% jump).
        status.progress = if session.program_streaming {
            let floor = session
                .last_progress
                .and_then(|progress| progress.path_distance_mm)
                .unwrap_or(0.0);
            let max_program_line =
                Self::progress_line_window(session.current_line, session.pending_ack.is_some());
            match session
                .toolpath
                .locate_forward(&work_position, floor, max_program_line)
            {
                Some(found) => {
                    session.last_progress = Some(found);
                    found
                }
                // No match in the window: keep the last reported part and only
                // move the tool marker.
                None => session
                    .last_progress
                    .map_or_else(ToolpathProgress::unavailable, |last| ToolpathProgress {
                        tool_position: Some(work_position.clone()),
                        ..last
                    }),
            }
        } else {
            session.last_progress = None;
            ToolpathProgress::unavailable()
        };
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
                    status.work_offset.a,
                    status.work_offset.b,
                    status.work_offset.c,
                ],
            );
        }
        // Remember how many position components the controller actually sends
        // so `machine_configuration` reports exactly the configured axes
        // (serial FluidNC: 3..6 axes — task: FluidNC over a COM port).
        if let Some(count) = Self::reported_axis_count(&response) {
            session.reported_axis_count = count;
        }

        Ok(status)
    }

    /// Reads `$$` settings and maps the GRBL $1xx/$3x/$13x numbering into
    /// per-axis travel/velocity/acceleration and spindle RPM limits.
    ///
    /// For serial FluidNC (task: FluidNC over a COM port) the configuration
    /// follows the WebSocket adapter's convention: $130-$135/$110-$115/
    /// $120-$125/$100-$105 are the X/Y/Z/A/B/C slots and the number of
    /// reported axes comes from the last `?` status poll, so a 4/5/6-axis
    /// machine exposes its rotary axes to the UI.
    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let mut session = self.session.lock().await;
        let settings = Self::query_settings(&mut session).await?;
        // Stock GRBL $32 distinguishes only spindle vs laser constant-power mode;
        // dynamic mode remains an explicit M4 G-code operation, not a $32 value.
        let spindle_mode = if self.protocol == SerialProtocol::Grbl
            && settings
                .get(&32)
                .is_some_and(|laser_mode| *laser_mode == 1.0)
        {
            crate::SpindleMode::LaserConstant
        } else {
            crate::SpindleMode::Standard
        };
        session.spindle_mode = spindle_mode;
        let travel = |number: u16| settings.get(&number).copied().filter(|value| *value > 0.0);
        let spindle_rpm =
            |number: u16| settings.get(&number).copied().filter(|value| *value >= 0.0);
        let min_rpm = spindle_rpm(31);
        let max_rpm =
            spindle_rpm(30).filter(|maximum| min_rpm.is_none_or(|minimum| minimum <= *maximum));
        session.min_spindle_rpm = min_rpm;
        session.max_spindle_rpm = max_rpm;

        let axis_definitions = [
            (MachineAxis::X, AxisUnits::Linear),
            (MachineAxis::Y, AxisUnits::Linear),
            (MachineAxis::Z, AxisUnits::Linear),
            (MachineAxis::A, AxisUnits::Angular),
            (MachineAxis::B, AxisUnits::Angular),
            (MachineAxis::C, AxisUnits::Angular),
        ];
        // GRBL boards are 3-axis by firmware design; serial FluidNC reports
        // its real axis count via the MPos/WPos width of the status line.
        let axis_count = match self.protocol {
            SerialProtocol::Grbl => 3,
            SerialProtocol::FluidNc => session.reported_axis_count.clamp(3, 6),
        };
        let axes = axis_definitions
            .into_iter()
            .take(axis_count)
            .enumerate()
            .map(|(index, (axis, units))| AxisConfiguration {
                axis,
                units,
                max_travel: travel(130 + index as u16),
                max_velocity: travel(110 + index as u16),
                acceleration: travel(120 + index as u16),
                steps_per_unit: travel(100 + index as u16),
            })
            .collect();

        let spindle = if spindle_mode == crate::SpindleMode::Standard {
            SpindleConfiguration {
                min_rpm,
                max_rpm,
                mode: spindle_mode,
                laser: None,
            }
        } else {
            let maximum = max_rpm.unwrap_or(1000.0);
            SpindleConfiguration {
                min_rpm: Some(0.0),
                max_rpm: Some(maximum),
                mode: spindle_mode,
                laser: Some(crate::LaserConfiguration {
                    min_power: 0.0,
                    max_power: maximum,
                    pwm_frequency_hz: None,
                    dynamic_power_supported: true,
                    auto_shutoff_on_pause: true,
                }),
            }
        };
        Ok(MachineConfiguration {
            axes,
            spindle,
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

    /// Changes GRBL `$32` between constant and dynamic laser behavior when the setting is available.
    async fn set_laser_mode(&self, mode: crate::SpindleMode) -> CncResult<()> {
        let value = match mode {
            crate::SpindleMode::LaserConstant => 1,
            crate::SpindleMode::LaserDynamic => 1,
            crate::SpindleMode::Standard => 0,
        };
        let mut session = self.session.lock().await;
        if session.protocol != SerialProtocol::Grbl {
            return Err(ControllerError::new(
                "unsupported",
                "runtime laser mode is not detected for this FluidNC firmware",
            ));
        }
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::send_command(&mut session, &format!("$32={value}")).await?;
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
