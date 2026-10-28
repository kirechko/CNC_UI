//! The machine-control surface: the single `impl LinuxCncApi for GrblAdapter` block,
//! delegating to the serial transport ([`super::transport`]) and the status parser
//! ([`super::status`]).

use super::{ContinuousXyJog, GrblAdapter};
use crate::{
    Axis, AxisConfiguration, AxisUnits, ControllerError, CncResult, CoordinateSystem, JogRequest,
    LinuxCncApi, MachineAxis, MachineConfiguration, MachineStatus, SpindleCommand,
    SpindleConfiguration, SpindleDirection,
};
use std::fs;
use std::path::Path;

impl LinuxCncApi for GrblAdapter {
    /// Polls the realtime status report; when the controller is idle and a continuous
    /// XY jog is pending, it pumps the next bounded `$J` chunk before parsing.
    fn status(&self) -> CncResult<MachineStatus> {
        let mut session = self.lock_session()?;
        let response = Self::query_status(&mut session)?;

        if response.starts_with("<Idle") {
            if let Some(jog) = session.continuous_xy_jog {
                if let Err(error) = Self::send_continuous_xy_chunk(&mut session, jog) {
                    session.continuous_xy_jog = None;
                    return Err(error);
                }
            }
        }

        Self::parse_status(&response, &session)
    }

    /// Reads the `$$` settings map and reports the X/Y/Z travel limits ($130-$132)
    /// and the spindle RPM range ($30-$31) as the machine configuration.
    fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let mut session = self.lock_session()?;
        let settings = Self::query_settings(&mut session)?;

        let travel = |setting: u16| {
            settings
                .get(&setting)
                .copied()
                .filter(|value| *value > 0.0)
        };
        let rpm = |setting: u16| {
            settings
                .get(&setting)
                .copied()
                .filter(|value| *value >= 0.0)
        };

        Ok(MachineConfiguration {
            axes: vec![
                AxisConfiguration {
                    axis: MachineAxis::X,
                    units: AxisUnits::Linear,
                    max_travel: travel(130),
                },
                AxisConfiguration {
                    axis: MachineAxis::Y,
                    units: AxisUnits::Linear,
                    max_travel: travel(131),
                },
                AxisConfiguration {
                    axis: MachineAxis::Z,
                    units: AxisUnits::Linear,
                    max_travel: travel(132),
                },
            ],
            spindle: SpindleConfiguration {
                min_rpm: rpm(31),
                max_rpm: rpm(30).filter(|maximum| {
                    settings
                        .get(&31)
                        .is_none_or(|minimum| minimum <= maximum)
                }),
            },
        })
    }

    /// Soft-reset/unlock command `$X`; GRBL has no separate power concept.
    fn power_on(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, "$X")
    }

    fn power_off(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "GRBL has no safe software power-off command; use the machine's hardware controls",
        ))
    }

    fn estop(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "GRBL serial commands are not an emergency stop; use the machine's hardware E-Stop",
        ))
    }

    /// Full-cycle homing `$H` (GRBL always homes every configured axis).
    fn home(&self, _axis: Option<Axis>) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, "$H")
    }

    /// Selects an active work coordinate system by sending its `G54`..`G59.3` modal
    /// code and remembering it for later zeroing commands and status display.
    fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, system.label())?;
        session.active_coordinate_system = system;
        Ok(())
    }

    /// Sets the selected system's origin to the current position with `G10 L20`.
    fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.lock_session()?;
        let command = format!("G10 L20 P{} X0 Y0 Z0", Self::coordinate_number(system));
        Self::send_command(&mut session, &command)?;
        session.active_coordinate_system = system;
        Ok(())
    }

    /// Zeros a single axis of the active system (per-axis `G10 L20` word).
    fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        let mut session = self.lock_session()?;
        let command = format!(
            "G10 L20 P{} {}0",
            Self::coordinate_number(session.active_coordinate_system),
            Self::format_axis(axis, 0.0)
        );
        Self::send_command(&mut session, &command)
    }

    /// Relative one-step jog via `$J=G91` with validated direction/distance/feed.
    fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        if ![-1, 1].contains(&request.direction)
            || !request.distance.is_finite()
            || request.distance <= 0.0
        {
            return Err(ControllerError::new("bad_jog", "invalid GRBL jog request"));
        }
        let feed_rate = request.feed_rate.unwrap_or(100.0);
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new("bad_jog", "jog feed rate must be positive"));
        }

        let distance = request.distance * f64::from(request.direction);
        let command = format!(
            "$J=G91 {} F{feed_rate:.0}",
            Self::format_axis(request.axis, distance)
        );
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, &command)
    }

    /// Starts (or stops, when `direction == 0`) continuous jogging on X/Y by
    /// installing a [`ContinuousXyJog`] pump; other axes are rejected.
    fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()> {
        let (x_direction, y_direction) = match axis {
            Axis::X => (direction, 0),
            Axis::Y => (0, direction),
            _ if direction == 0 => {
                let mut session = self.lock_session()?;
                session.continuous_xy_jog = None;
                return Self::send_realtime(&mut session, 0x85);
            }
            _ => {
                return Err(ControllerError::new(
                    "unsupported",
                    "GRBL continuous jog currently supports X and Y axes",
                ));
            }
        };
        self.jog_continuous_xy(x_direction, y_direction, feed_rate)
    }

    /// Cancels any pending motion with the realtime feed-cancel byte 0x85.
    fn cancel_jog(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        session.continuous_xy_jog = None;
        Self::send_realtime(&mut session, 0x85)
    }

    /// Diagonal continuous XY jog: validates the direction pair, stores the pump
    /// state, and immediately sends the first chunk (zero vectors act as a cancel).
    fn jog_continuous_xy(
        &self,
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        if ![-1, 0, 1].contains(&x_direction)
            || ![-1, 0, 1].contains(&y_direction)
            || !feed_rate.is_finite()
            || feed_rate <= 0.0
        {
            return Err(ControllerError::new("bad_jog", "invalid continuous XY jog"));
        }

        let mut session = self.lock_session()?;
        if x_direction == 0 && y_direction == 0 {
            session.continuous_xy_jog = None;
            return Self::send_realtime(&mut session, 0x85);
        }

        let jog = ContinuousXyJog {
            x_direction,
            y_direction,
            feed_rate,
        };
        session.continuous_xy_jog = Some(jog);
        if let Err(error) = Self::send_continuous_xy_chunk(&mut session, jog) {
            session.continuous_xy_jog = None;
            return Err(error);
        }
        Ok(())
    }

    /// Sends an arbitrary single G-code line as-is (manual data entry).
    fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, command)
    }

    /// Reads a program from the host file system, trims/drops blank lines and keeps
    /// it in memory for later stop-and-wait streaming.
    fn load_program(&self, path: &Path) -> CncResult<()> {
        let content =
            fs::read_to_string(path).map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        let lines = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("program.nc")
            .to_string();

        let mut session = self.lock_session()?;
        session.program = lines;
        session.program_name = Some(name);
        session.current_line = 0;
        Ok(())
    }

    /// Streams the in-memory program line by line (stop-and-wait) starting at
    /// `start_line`, updating the current-line bookkeeping after each acknowledgement.
    fn program_run(&self, start_line: u32) -> CncResult<()> {
        let mut session = self.lock_session()?;
        let start = start_line as usize;
        if start > session.program.len() {
            return Err(ControllerError::new(
                "bad_program",
                "start line is outside the loaded GRBL program",
            ));
        }

        for index in start..session.program.len() {
            let line = session.program[index].clone();
            Self::send_command(&mut session, &line)?;
            session.current_line = (index + 1) as u32;
        }
        Ok(())
    }

    /// Realtime feed hold (`!`).
    fn program_pause(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_realtime(&mut session, b'!')
    }

    /// Resumes a feed hold with the realtime `~` byte.
    fn program_resume(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_realtime(&mut session, b'~')
    }

    /// Aborts the running program with the realtime soft-stop byte 0x18 and resets
    /// the host-side line counter.
    fn program_abort(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_realtime(&mut session, 0x18)?;
        session.current_line = 0;
        Ok(())
    }

    /// Starts/stops the spindle with `M3`/`M4 S..`/`M5` and records the setpoint
    /// so the status report can show the commanded RPM.
    fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        if !command.rpm.is_finite() || command.rpm < 0.0 {
            return Err(ControllerError::new(
                "bad_spindle",
                "spindle RPM must be finite and non-negative",
            ));
        }

        let mut session = self.lock_session()?;
        let gcode = match command.direction {
            SpindleDirection::Stopped => "M5".to_string(),
            SpindleDirection::Clockwise => format!("M3 S{:.0}", command.rpm),
            SpindleDirection::CounterClockwise => format!("M4 S{:.0}", command.rpm),
        };
        Self::send_command(&mut session, &gcode)?;
        session.spindle_command = command;
        Ok(())
    }

    /// Clears an ALARM state with the unlock command `$X`.
    fn clear_error(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, "$X")
    }
}

