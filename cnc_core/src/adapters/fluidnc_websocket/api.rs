//! The `CncApi` surface of [`FluidNcWebSocketAdapter`]. Rust allows only one
//! trait-impl block per type (E0119), so all trait methods live in this file.
//! Expensive bodies (status collection, rediscovery streaming, machine
//! configuration, program loading) delegate to inherent helpers in the
//! sibling modules; the remaining methods are thin command transmitters.

use super::{ContinuousJog, FluidNcWebSocketAdapter};
use crate::discovery::DiscoveredDevice;
use crate::{
    CncApi, CncEvent, CncResult, ControllerError, CoordinateSystem, JogRequest,
    MachineConfiguration, MachineStatus, OverrideKind, ProgramSource, SpindleCommand,
    SpindleDirection,
};
use std::path::Path;
use tokio::sync::{broadcast, watch};

#[async_trait::async_trait]
impl CncApi for FluidNcWebSocketAdapter {
    fn discovered_devices(&self) -> Vec<DiscoveredDevice> {
        FluidNcWebSocketAdapter::discovered_devices(self)
    }

    async fn rediscover_devices_stream(
        &self,
        priority_targets: Vec<String>,
    ) -> CncResult<tokio::sync::mpsc::Receiver<DiscoveredDevice>> {
        self.start_rediscovery_stream(priority_targets).await
    }

    async fn rediscover_devices(&self) -> CncResult<Vec<DiscoveredDevice>> {
        let mut discoveries = self.rediscover_devices_stream(Vec::new()).await?;
        let mut devices = Vec::new();
        while let Some(device) = discoveries.recv().await {
            devices.push(device);
        }
        Ok(devices)
    }

    fn connected_device_id(&self) -> Option<String> {
        FluidNcWebSocketAdapter::connected_device_id(self)
    }

    async fn status(&self) -> CncResult<MachineStatus> {
        self.collect_status().await
    }

    fn last_status(&self) -> Option<MachineStatus> {
        Some(self.status_tx.borrow().clone())
    }

    fn subscribe_status(&self) -> Option<watch::Receiver<MachineStatus>> {
        Some(self.status_tx.subscribe())
    }

    fn subscribe_events(&self) -> Option<broadcast::Receiver<CncEvent>> {
        Some(self.events.subscribe())
    }

    async fn load_program_source(&self, source: ProgramSource) -> CncResult<()> {
        match source {
            ProgramSource::Path(path) => self.load_program_file(&path).await,
            ProgramSource::Inline(content) => self.load_inline_program(content).await,
        }
    }

    async fn set_override(&self, kind: OverrideKind, value: f64) -> CncResult<()> {
        let _ = (kind, value);
        Err(ControllerError::new(
            "unsupported",
            "FluidNC override commands are not enabled until their controller-specific behavior is verified",
        ))
    }

    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.machine_configuration_snapshot(&mut session).await
    }

    async fn power_on(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, "$X").await
    }

    async fn power_off(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "FluidNC has no safe software power-off command; use the machine hardware controls",
        ))
    }

    async fn estop(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "WebSocket commands are not an emergency stop; use the machine hardware E-Stop",
        ))
    }

    async fn home(&self, axis: Option<crate::Axis>) -> CncResult<()> {
        if axis.is_some() {
            return Err(ControllerError::new(
                "unsupported",
                "FluidNC $H homes all configured axes; individual-axis homing is unsupported",
            ));
        }
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, "$H").await
    }

    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, system.label()).await?;
        session.active_coordinate_system = system;
        Ok(())
    }

    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let axes = ['X', 'Y', 'Z', 'A', 'B', 'C']
            .into_iter()
            .take(session.reported_axis_count)
            .map(|axis| format!("{axis}0"))
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!(
            "G10 L20 P{} {axes}",
            FluidNcWebSocketAdapter::coordinate_number(system)
        );
        self.send_command(&mut session, &command).await?;
        session.work_offset = None;
        Ok(())
    }

    async fn set_axis_work_zero(&self, axis: crate::Axis) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let command = format!(
            "G10 L20 P{} {}0",
            FluidNcWebSocketAdapter::coordinate_number(session.active_coordinate_system),
            FluidNcWebSocketAdapter::axis_letter(axis),
        );
        self.send_command(&mut session, &command).await?;
        session.work_offset = None;
        Ok(())
    }

    async fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        if ![-1, 1].contains(&request.direction)
            || !request.distance.is_finite()
            || request.distance <= 0.0
        {
            return Err(ControllerError::new(
                "bad_jog",
                "invalid FluidNC jog request",
            ));
        }
        let feed_rate = request.feed_rate.unwrap_or(100.0);
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }

        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let command = format!(
            "$J=G21 G91 {}{:.4} F{feed_rate}",
            FluidNcWebSocketAdapter::axis_letter(request.axis),
            request.distance * f64::from(request.direction),
        );
        self.send_command(&mut session, &command).await
    }

    async fn jog_continuous(
        &self,
        axis: crate::Axis,
        direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        if ![-1, 0, 1].contains(&direction) {
            return Err(ControllerError::new(
                "bad_jog",
                "invalid continuous jog direction",
            ));
        }
        if direction == 0 {
            return self.cancel_jog().await;
        }
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }

        let jog = ContinuousJog {
            x: direction,
            y: 0,
            axis: Some(axis),
            feed_rate,
        };
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.continuous_jog = Some(jog);
        self.send_jog_chunk(&mut session, jog).await
    }

    async fn jog_continuous_xy(
        &self,
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        if ![-1, 0, 1].contains(&x_direction)
            || ![-1, 0, 1].contains(&y_direction)
            || !feed_rate.is_finite()
            || (x_direction != 0 || y_direction != 0) && feed_rate <= 0.0
        {
            return Err(ControllerError::new("bad_jog", "invalid continuous XY jog"));
        }
        if x_direction == 0 && y_direction == 0 {
            return self.cancel_jog().await;
        }

        let jog = ContinuousJog {
            x: x_direction,
            y: y_direction,
            axis: None,
            feed_rate,
        };
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.continuous_jog = Some(jog);
        self.send_jog_chunk(&mut session, jog).await
    }

    async fn cancel_jog(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.continuous_jog = None;
        self.send_realtime(&mut session, 0x85).await
    }

    async fn stop_motion(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let jogging = session.continuous_jog.take().is_some();
        self.send_realtime(&mut session, if jogging { 0x85 } else { b'!' })
            .await
    }

    async fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, command).await
    }

    async fn load_program(&self, path: &Path) -> CncResult<()> {
        self.load_program_file(path).await
    }

    async fn program_run(&self, start_line: u32) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let status = self.query_status(&mut session).await?;
        let machine_state = status
            .strip_prefix('<')
            .and_then(|value| value.split('|').next())
            .unwrap_or("");
        if !machine_state.eq_ignore_ascii_case("Idle") {
            return Err(ControllerError::new(
                "motion_busy",
                format!("cannot start a FluidNC program while state is {machine_state}"),
            ));
        }

        let start = start_line as usize;
        if session.program.is_empty() || start >= session.program.len() {
            return Err(ControllerError::new(
                "bad_program",
                "no program loaded or start line is outside the loaded FluidNC program",
            ));
        }

        session.current_line = start_line;
        session.active_program_line = None;
        session.program_acknowledgements.clear();
        session.program_acknowledged_bytes = 0;
        session.program_error = None;
        session.program_streaming = true;
        // A new run must not inherit the progress floor from a previous run.
        session.last_progress = None;
        self.fill_program_window(&mut session).await?;
        let _ = self.events.send(CncEvent::ProgramStarted);
        Ok(())
    }

    async fn program_pause(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_realtime(&mut session, b'!').await
    }

    async fn program_resume(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_realtime(&mut session, b'~').await
    }

    async fn program_abort(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.program_streaming = false;
        session.continuous_jog = None;
        session.current_line = 0;
        // Abort ends the run: drop the progress floor so the next run starts clean.
        session.last_progress = None;
        session.active_program_line = None;
        session.program_acknowledgements.clear();
        session.program_acknowledged_bytes = 0;
        // FluidNC reports ALARM:3 (Abort Cycle) after Ctrl-X. This is the expected
        // response to Stop, but the controller may still remain in the Alarm state;
        // do not suppress it or assume that $X has already been executed.
        self.send_realtime(&mut session, 0x18).await
    }

    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        if !command.rpm.is_finite() || command.rpm < 0.0 {
            return Err(ControllerError::new(
                "bad_spindle",
                "spindle RPM must be finite and non-negative",
            ));
        }

        let (gcode, normalized) = match command.direction {
            _ if command.rpm == 0.0 => (
                "M5".to_owned(),
                SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
            ),
            SpindleDirection::Stopped => (
                "M5".to_owned(),
                SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
            ),
            SpindleDirection::Clockwise => (format!("M3 S{}", command.rpm), command),
            SpindleDirection::CounterClockwise => (format!("M4 S{}", command.rpm), command),
        };

        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.send_command(&mut session, &gcode).await?;
        session.spindle_command = normalized;
        Ok(())
    }

    async fn clear_error(&self) -> CncResult<()> {
        self.power_on().await
    }
}
