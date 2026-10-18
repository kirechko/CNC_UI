use crate::cnc_api::{
    Axis, AxisConfiguration, AxisUnits, CncResult, ControllerError, CoordinateSystem,
    InterpreterState, JogRequest, LinuxCncApi, MachineAxis, MachineConfiguration, MachineStatus,
    Position, SpindleCommand, SpindleConfiguration, SpindleDirection, SpindleStatus, TaskMode,
    TaskState,
};
use serialport::SerialPort;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
const STATUS_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RESPONSE_LINE: usize = 4096;

#[derive(Clone, Copy)]
struct ContinuousXyJog {
    x_direction: i8,
    y_direction: i8,
    feed_rate: f64,
}

struct Session {
    port: Box<dyn SerialPort>,
    program: Vec<String>,
    program_name: Option<String>,
    current_line: u32,
    active_coordinate_system: CoordinateSystem,
    spindle_command: SpindleCommand,
    continuous_xy_jog: Option<ContinuousXyJog>,
}

/// GRBL 1.1 Serial adapter. E-Stop must be provided by the machine's hardware;
/// GRBL's real-time feed hold is deliberately not presented as an E-Stop.
pub struct GrblAdapter {
    session: Mutex<Session>,
}

impl GrblAdapter {
    pub fn connect(port_name: &str, baud_rate: u32) -> CncResult<Self> {
        let port = serialport::new(port_name, baud_rate)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|error| ControllerError::new("connect", error.to_string()))?;

        Ok(Self {
            session: Mutex::new(Session {
                port,
                program: Vec::new(),
                program_name: None,
                current_line: 0,
                active_coordinate_system: CoordinateSystem::G54,
                spindle_command: SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
                continuous_xy_jog: None,
            }),
        })
    }

    fn lock_session(&self) -> CncResult<std::sync::MutexGuard<'_, Session>> {
        self.session
            .lock()
            .map_err(|_| ControllerError::new("connection", "GRBL session lock poisoned"))
    }

    fn read_line(port: &mut dyn SerialPort, deadline: Instant) -> CncResult<String> {
        let mut bytes = Vec::new();

        while Instant::now() < deadline {
            let mut byte = [0_u8; 1];
            match port.read(&mut byte) {
                Ok(0) => continue,
                Ok(_) if byte[0] == b'\n' => {
                    return Ok(String::from_utf8_lossy(&bytes).trim().to_string());
                }
                Ok(_) if byte[0] != b'\r' => {
                    bytes.push(byte[0]);
                    if bytes.len() > MAX_RESPONSE_LINE {
                        return Err(ControllerError::new(
                            "protocol",
                            "GRBL response line exceeds size limit",
                        ));
                    }
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => {
                    return Err(ControllerError::new("io", error.to_string()));
                }
            }
        }

        Err(ControllerError::new("timeout", "timed out waiting for GRBL response"))
    }

    fn send_command(session: &mut Session, command: &str) -> CncResult<()> {
        if command.contains('\n') || command.contains('\r') {
            return Err(ControllerError::new(
                "bad_command",
                "a GRBL command must contain exactly one line",
            ));
        }
        if command.trim().is_empty() {
            return Ok(());
        }

        session
            .port
            .write_all(command.trim().as_bytes())
            .and_then(|_| session.port.write_all(b"\n"))
            .and_then(|_| session.port.flush())
            .map_err(|error| ControllerError::new("io", error.to_string()))?;

        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            let line = Self::read_line(&mut *session.port, deadline)?;
            if line == "ok" {
                return Ok(());
            }
            if let Some(error) = line.strip_prefix("error:") {
                return Err(ControllerError::new(
                    "gcode_error",
                    format!("GRBL rejected command ({error}): {command}"),
                ));
            }
            if line.starts_with("ALARM:") {
                return Err(ControllerError::new("alarm", line));
            }
            // Startup banners and asynchronous status reports are not command ACKs.
        }
    }

    fn send_realtime(session: &mut Session, byte: u8) -> CncResult<()> {
        session
            .port
            .write_all(&[byte])
            .and_then(|_| session.port.flush())
            .map_err(|error| ControllerError::new("io", error.to_string()))
    }

    fn send_continuous_xy_chunk(
        session: &mut Session,
        jog: ContinuousXyJog,
    ) -> CncResult<()> {
        // Ограниченные короткие $J-блоки дают непрерывное движение, пока UI
        // удерживает команду; GRBL штатно отменяет текущий блок realtime-командой 0x85.
        const CHUNK_LENGTH_MM: f64 = 10.0;
        let norm = f64::from(
            jog.x_direction * jog.x_direction + jog.y_direction * jog.y_direction,
        )
        .sqrt();
        let x = CHUNK_LENGTH_MM * f64::from(jog.x_direction) / norm;
        let y = CHUNK_LENGTH_MM * f64::from(jog.y_direction) / norm;
        let command = format!(
            "$J=G91 X{x:.4} Y{y:.4} F{:.0}",
            jog.feed_rate
        );
        Self::send_command(session, &command)
    }

    fn query_status(session: &mut Session) -> CncResult<String> {
        session
            .port
            .write_all(b"?")
            .and_then(|_| session.port.flush())
            .map_err(|error| ControllerError::new("io", error.to_string()))?;

        let deadline = Instant::now() + STATUS_TIMEOUT;
        loop {
            let line = Self::read_line(&mut *session.port, deadline)?;
            if line.starts_with('<') && line.ends_with('>') {
                return Ok(line);
            }
        }
    }

    fn query_settings(session: &mut Session) -> CncResult<HashMap<u16, f64>> {
        session
            .port
            .write_all(b"$$\n")
            .and_then(|_| session.port.flush())
            .map_err(|error| ControllerError::new("io", error.to_string()))?;

        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        let mut settings = HashMap::new();

        loop {
            let line = Self::read_line(&mut *session.port, deadline)?;
            if line == "ok" {
                return Ok(settings);
            }
            if let Some(error) = line.strip_prefix("error:") {
                return Err(ControllerError::new("settings", error));
            }
            if line.starts_with("ALARM:") {
                return Err(ControllerError::new("alarm", line));
            }

            let Some(setting) = line.strip_prefix('$') else {
                continue;
            };
            let Some((number, value)) = setting.split_once('=') else {
                continue;
            };
            let (Ok(number), Ok(value)) = (number.parse::<u16>(), value.parse::<f64>()) else {
                continue;
            };
            if value.is_finite() {
                settings.insert(number, value);
            }
        }
    }

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

    fn format_axis(axis: Axis, value: f64) -> String {
        let letter = match axis {
            Axis::X => 'X',
            Axis::Y => 'Y',
            Axis::Z => 'Z',
            Axis::A => 'A',
            Axis::B => 'B',
        };
        format!("{letter}{value:.3}")
    }

    fn parse_triplet(value: &str) -> Option<[f64; 3]> {
        let mut values = value.split(',').map(str::parse::<f64>);
        let triplet = [
            values.next()?.ok()?,
            values.next()?.ok()?,
            values.next()?.ok()?,
        ];
        if values.next().is_some() || triplet.iter().any(|value| !value.is_finite()) {
            return None;
        }
        Some(triplet)
    }

    fn parse_status(
        response: &str,
        session: &Session,
    ) -> CncResult<MachineStatus> {
        let body = response
            .strip_prefix('<')
            .and_then(|value| value.strip_suffix('>'))
            .ok_or_else(|| ControllerError::new("protocol", "malformed GRBL status response"))?;

        let mut fields = body.split('|');
        let state = fields.next().unwrap_or("Unknown");
        let mut machine_position = None;
        let mut work_position = None;
        let mut work_offset = None;
        let mut actual_rpm = 0.0;
        let mut feed_rate = 0.0;

        for field in fields {
            if let Some(value) = field.strip_prefix("MPos:") {
                machine_position = Self::parse_triplet(value);
            } else if let Some(value) = field.strip_prefix("WPos:") {
                work_position = Self::parse_triplet(value);
            } else if let Some(value) = field.strip_prefix("WCO:") {
                work_offset = Self::parse_triplet(value);
            } else if let Some(value) = field.strip_prefix("FS:") {
                let mut values = value.split(',');
                feed_rate = values
                    .next()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0);
                actual_rpm = values
                    .next()
                    .and_then(|value| value.parse::<f64>().ok())
                    .filter(|value| value.is_finite())
                    .unwrap_or(0.0);
            }
        }

        let offset = work_offset.unwrap_or([0.0; 3]);
        let machine = machine_position
            .or_else(|| work_position.map(|work| [work[0] + offset[0], work[1] + offset[1], work[2] + offset[2]]))
            .unwrap_or([0.0; 3]);
        let work = work_position.unwrap_or([
            machine[0] - offset[0],
            machine[1] - offset[1],
            machine[2] - offset[2],
        ]);

        let state_lower = state.to_ascii_lowercase();
        let is_alarm = state_lower.starts_with("alarm");
        let is_running = state_lower.starts_with("run");
        let is_paused = state_lower.starts_with("hold");
        let homing = state_lower.starts_with("home");

        Ok(MachineStatus {
            connected: true,
            task_mode: if is_running {
                TaskMode::Auto
            } else {
                TaskMode::Manual
            },
            task_state: if is_alarm { TaskState::Estop } else { TaskState::On },
            interpreter_state: if is_running {
                InterpreterState::Reading
            } else if is_paused {
                InterpreterState::Paused
            } else {
                InterpreterState::Idle
            },
            position: Position {
                x: machine[0],
                y: machine[1],
                z: machine[2],
                a: 0.0,
                b: 0.0,
            },
            work_offset: Position {
                x: offset[0],
                y: offset[1],
                z: offset[2],
                a: 0.0,
                b: 0.0,
            },
            active_coordinate_system: session.active_coordinate_system,
            coordinate_systems: Vec::new(),
            feed_rate,
            spindle: SpindleStatus {
                commanded_rpm: session.spindle_command.rpm,
                actual_rpm,
                direction: session.spindle_command.direction,
                enabled: actual_rpm > 0.0 || session.spindle_command.rpm > 0.0,
            },
            mist_coolant_on: false,
            flood_coolant_on: false,
            max_spindle_rpm: 12_000.0,
            loaded_program: session.program_name.clone(),
            current_line: session.current_line,
            total_lines: session.program.len() as u32,
            message: Some(format!("GRBL {state}; work position {:.3}, {:.3}, {:.3}", work[0], work[1], work[2])),
            homed: [false; 5],
            homing,
        })
    }
}

impl LinuxCncApi for GrblAdapter {
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

    fn home(&self, _axis: Option<Axis>) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, "$H")
    }

    fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, system.label())?;
        session.active_coordinate_system = system;
        Ok(())
    }

    fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.lock_session()?;
        let command = format!("G10 L20 P{} X0 Y0 Z0", Self::coordinate_number(system));
        Self::send_command(&mut session, &command)?;
        session.active_coordinate_system = system;
        Ok(())
    }

    fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        let mut session = self.lock_session()?;
        let command = format!(
            "G10 L20 P{} {}0",
            Self::coordinate_number(session.active_coordinate_system),
            Self::format_axis(axis, 0.0)
        );
        Self::send_command(&mut session, &command)
    }

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

    fn cancel_jog(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        session.continuous_xy_jog = None;
        Self::send_realtime(&mut session, 0x85)
    }

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

    fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, command)
    }

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

    fn program_pause(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_realtime(&mut session, b'!')
    }

    fn program_resume(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_realtime(&mut session, b'~')
    }

    fn program_abort(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_realtime(&mut session, 0x18)?;
        session.current_line = 0;
        Ok(())
    }

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

    fn clear_error(&self) -> CncResult<()> {
        let mut session = self.lock_session()?;
        Self::send_command(&mut session, "$X")
    }
}
