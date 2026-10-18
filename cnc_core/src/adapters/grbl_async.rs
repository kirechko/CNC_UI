use crate::{
    Axis, AxisConfiguration, AxisUnits, CncResult, ControllerError, CoordinateSystem,
    InterpreterState, JogRequest, LinuxCncApi, MachineAxis, MachineConfiguration, MachineStatus,
    Position, SpindleCommand, SpindleConfiguration, SpindleDirection, SpindleStatus, TaskMode,
    TaskState,
};
use std::{collections::HashMap, path::Path, time::Duration};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
    time::timeout,
};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
const STATUS_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RESPONSE_LINE: usize = 4096;
const JOG_CHUNK_MM: f64 = 10.0;

#[derive(Clone, Copy)]
struct ContinuousJog {
    x_direction: i8,
    y_direction: i8,
    axis: Option<Axis>,
    feed_rate: f64,
}

struct Session {
    port: SerialStream,
    rx_buffer: Vec<u8>,
    last_work_offset: Option<[f64; 3]>,
    program: Vec<String>,
    program_name: Option<String>,
    current_line: u32,
    active_coordinate_system: CoordinateSystem,
    spindle_command: SpindleCommand,
    continuous_jog: Option<ContinuousJog>,
}

/// Асинхронный GRBL 1.1 Serial-адаптер.
///
/// E-Stop не эмулируется программной командой: аварийный останов должен
/// выполняться аппаратной цепью станка.
pub struct GrblAdapter {
    session: Mutex<Session>,
}

impl GrblAdapter {
    pub fn connect(port_name: &str, baud_rate: u32) -> CncResult<Self> {
        let port = tokio_serial::new(port_name, baud_rate)
            .open_native_async()
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
                continuous_jog: None,
                rx_buffer: Vec::new(),
                last_work_offset: None,
            }),
        })
    }

    fn extract_line(rx_buffer: &mut Vec<u8>) -> CncResult<Option<String>> {
        if let Some(line_end) = rx_buffer.iter().position(|byte| *byte == b'\n') {
            if line_end > MAX_RESPONSE_LINE {
                return Err(ControllerError::new(
                    "protocol",
                    "GRBL response line exceeds size limit",
                ));
            }

            let line: Vec<u8> = rx_buffer.drain(..=line_end).collect();
            return Ok(Some(
                String::from_utf8_lossy(&line[..line.len() - 1])
                    .trim()
                    .to_string(),
            ));
        }

        if rx_buffer.len() > MAX_RESPONSE_LINE {
            return Err(ControllerError::new(
                "protocol",
                "GRBL response line exceeds size limit",
            ));
        }

        Ok(None)
    }

    async fn read_line(port: &mut SerialStream, rx_buffer: &mut Vec<u8>) -> CncResult<String> {
        loop {
            if let Some(line) = Self::extract_line(rx_buffer)? {
                return Ok(line);
            }

            let mut chunk = [0_u8; 256];
            let bytes_read = port
                .read(&mut chunk)
                .await
                .map_err(|error| ControllerError::new("io", error.to_string()))?;
            if bytes_read == 0 {
                return Err(ControllerError::new(
                    "disconnected",
                    "GRBL serial port closed",
                ));
            }
            rx_buffer.extend_from_slice(&chunk[..bytes_read]);
        }
    }

    async fn send_command(session: &mut Session, command: &str) -> CncResult<()> {
        if command.contains(['\n', '\r']) {
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
            .await
            .map_err(|error| ControllerError::new("io", error.to_string()))?;
        session
            .port
            .write_all(b"\n")
            .await
            .map_err(|error| ControllerError::new("io", error.to_string()))?;

        timeout(RESPONSE_TIMEOUT, async {
            loop {
                let line = Self::read_line(&mut session.port, &mut session.rx_buffer).await?;
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
                // Игнорируем startup banner и асинхронные status reports.
            }
        })
        .await
        .map_err(|_| {
            ControllerError::new("timeout", "timed out waiting for GRBL acknowledgement")
        })?
    }

    async fn send_realtime(session: &mut Session, byte: u8) -> CncResult<()> {
        session
            .port
            .write_all(&[byte])
            .await
            .map_err(|error| ControllerError::new("io", error.to_string()))
    }

    async fn send_jog_chunk(session: &mut Session, jog: ContinuousJog) -> CncResult<()> {
        let command = if let Some(axis) = jog.axis {
            let letter = Self::axis_letter(axis);
            format!(
                "$J=G91 {letter}{:.4} F{:.0}",
                JOG_CHUNK_MM * f64::from(jog.x_direction),
                jog.feed_rate
            )
        } else {
            let norm =
                f64::from(jog.x_direction * jog.x_direction + jog.y_direction * jog.y_direction)
                    .sqrt();
            let x = JOG_CHUNK_MM * f64::from(jog.x_direction) / norm;
            let y = JOG_CHUNK_MM * f64::from(jog.y_direction) / norm;
            format!("$J=G91 X{x:.4} Y{y:.4} F{:.0}", jog.feed_rate)
        };

        Self::send_command(session, &command).await
    }

    async fn query_status(session: &mut Session) -> CncResult<String> {
        session
            .port
            .write_all(b"?")
            .await
            .map_err(|error| ControllerError::new("io", error.to_string()))?;

        timeout(STATUS_TIMEOUT, async {
            loop {
                let line = Self::read_line(&mut session.port, &mut session.rx_buffer).await?;
                if line.starts_with('<') && line.ends_with('>') {
                    return Ok(line);
                }
            }
        })
        .await
        .map_err(|_| ControllerError::new("timeout", "timed out waiting for GRBL status"))?
    }

    async fn query_settings(session: &mut Session) -> CncResult<HashMap<u16, f64>> {
        session
            .port
            .write_all(b"$$\n")
            .await
            .map_err(|error| ControllerError::new("io", error.to_string()))?;

        timeout(RESPONSE_TIMEOUT, async {
            let mut settings = HashMap::new();

            loop {
                let line = Self::read_line(&mut session.port, &mut session.rx_buffer).await?;
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
        })
        .await
        .map_err(|_| ControllerError::new("timeout", "timed out waiting for GRBL settings"))?
    }

    fn parse_triplet(value: &str) -> Option<[f64; 3]> {
        let mut values = value.split(',').map(str::parse::<f64>);
        let result = [
            values.next()?.ok()?,
            values.next()?.ok()?,
            values.next()?.ok()?,
        ];
        if values.next().is_some() || result.iter().any(|value| !value.is_finite()) {
            return None;
        }
        Some(result)
    }

    fn parse_status(
        response: &str,
        cached_work_offset: Option<[f64; 3]>,
        active_coordinate_system: CoordinateSystem,
        spindle_command: SpindleCommand,
        program_name: Option<&str>,
        current_line: u32,
        total_lines: u32,
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
        let mut feed_rate = 0.0;
        let mut actual_rpm = 0.0;

        for field in fields {
            if let Some(value) = field.strip_prefix("MPos:") {
                machine_position =
                    Some(Self::parse_triplet(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL MPos field")
                    })?);
            } else if let Some(value) = field.strip_prefix("WPos:") {
                work_position =
                    Some(Self::parse_triplet(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL WPos field")
                    })?);
            } else if let Some(value) = field.strip_prefix("WCO:") {
                work_offset =
                    Some(Self::parse_triplet(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL WCO field")
                    })?);
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

        let offset = work_offset
            .or_else(|| match (machine_position, work_position) {
                (Some(machine), Some(work)) => Some([
                    machine[0] - work[0],
                    machine[1] - work[1],
                    machine[2] - work[2],
                ]),
                _ => None,
            })
            .or(cached_work_offset)
            .unwrap_or([0.0; 3]);
        let machine = machine_position
            .or_else(|| {
                work_position.map(|work| {
                    [
                        work[0] + offset[0],
                        work[1] + offset[1],
                        work[2] + offset[2],
                    ]
                })
            })
            .unwrap_or([0.0; 3]);
        let state_lower = state.to_ascii_lowercase();
        let alarm = state_lower.starts_with("alarm");
        let running = state_lower.starts_with("run");
        let jogging = state_lower.starts_with("jog");
        let paused = state_lower.starts_with("hold");
        let homing = state_lower.starts_with("home");

        Ok(MachineStatus {
            connected: true,
            task_mode: if running {
                TaskMode::Auto
            } else {
                TaskMode::Manual
            },
            task_state: if alarm {
                TaskState::Estop
            } else {
                TaskState::On
            },
            interpreter_state: if running {
                InterpreterState::Reading
            } else if paused {
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
            active_coordinate_system,
            coordinate_systems: Vec::new(),
            feed_rate: if jogging { 0.0 } else { feed_rate },
            jog_rate: if jogging { feed_rate } else { 0.0 },
            motion_mode: if jogging {
                Some("Jog".into())
            } else if running {
                Some("G-code".into())
            } else {
                None
            },
            spindle: SpindleStatus {
                commanded_rpm: spindle_command.rpm,
                actual_rpm,
                direction: spindle_command.direction,
                enabled: actual_rpm > 0.0 || spindle_command.rpm > 0.0,
            },
            mist_coolant_on: false,
            flood_coolant_on: false,
            max_spindle_rpm: 12_000.0,
            loaded_program: program_name.map(str::to_owned),
            current_line,
            total_lines,
            message: Some(format!("GRBL {state}")),
            homed: [false; 5],
            homing,
        })
    }

    fn ensure_supported_axis(axis: Axis) -> CncResult<()> {
        match axis {
            Axis::X | Axis::Y | Axis::Z => Ok(()),
            Axis::A | Axis::B => Err(ControllerError::new(
                "unsupported_axis",
                format!("GRBL 1.1 does not support the {axis:?} rotary axis"),
            )),
        }
    }

    fn axis_letter(axis: Axis) -> char {
        match axis {
            Axis::X => 'X',
            Axis::Y => 'Y',
            Axis::Z => 'Z',
            Axis::A => 'A',
            Axis::B => 'B',
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
}

#[async_trait::async_trait]
impl LinuxCncApi for GrblAdapter {
    async fn status(&self) -> CncResult<MachineStatus> {
        let mut session = self.session.lock().await;
        let response = Self::query_status(&mut session).await?;

        if response.starts_with("<Idle") {
            if let Some(jog) = session.continuous_jog {
                if let Err(error) = Self::send_jog_chunk(&mut session, jog).await {
                    session.continuous_jog = None;
                    return Err(error);
                }
            }
        }

        let status = Self::parse_status(
            &response,
            session.last_work_offset,
            session.active_coordinate_system,
            session.spindle_command,
            session.program_name.as_deref(),
            session.current_line,
            session.program.len() as u32,
        )?;

        if response.contains("|WCO:")
            || (response.contains("|MPos:") && response.contains("|WPos:"))
        {
            session.last_work_offset = Some([
                status.work_offset.x,
                status.work_offset.y,
                status.work_offset.z,
            ]);
        }

        Ok(status)
    }

    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let mut session = self.session.lock().await;
        let settings = Self::query_settings(&mut session).await?;
        let travel = |number: u16| settings.get(&number).copied().filter(|value| *value > 0.0);
        let spindle_rpm =
            |number: u16| settings.get(&number).copied().filter(|value| *value >= 0.0);

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
                min_rpm: spindle_rpm(31),
                max_rpm: spindle_rpm(30)
                    .filter(|maximum| settings.get(&31).is_none_or(|minimum| minimum <= maximum)),
            },
        })
    }

    async fn power_on(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
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
        Self::send_command(&mut session, "$H").await
    }

    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::send_command(&mut session, system.label()).await?;
        session.active_coordinate_system = system;
        Ok(())
    }

    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.session.lock().await;
        let command = format!("G10 L20 P{} X0 Y0 Z0", Self::coordinate_number(system));
        Self::send_command(&mut session, &command).await?;
        session.active_coordinate_system = system;
        Ok(())
    }

    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        let mut session = self.session.lock().await;
        let command = format!(
            "G10 L20 P{} {}0",
            Self::coordinate_number(session.active_coordinate_system),
            Self::axis_letter(axis)
        );
        Self::send_command(&mut session, &command).await
    }

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
        let command = format!(
            "$J=G91 {}{value:.4} F{feed_rate:.0}",
            Self::axis_letter(request.axis)
        );
        let mut session = self.session.lock().await;
        Self::send_command(&mut session, &command).await
    }

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

        let jog = ContinuousJog {
            x_direction: direction,
            y_direction: 0,
            axis: Some(axis),
            feed_rate,
        };
        let mut session = self.session.lock().await;
        session.continuous_jog = Some(jog);
        if let Err(error) = Self::send_jog_chunk(&mut session, jog).await {
            session.continuous_jog = None;
            return Err(error);
        }
        Ok(())
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
            || feed_rate <= 0.0
        {
            return Err(ControllerError::new("bad_jog", "invalid continuous XY jog"));
        }
        if x_direction == 0 && y_direction == 0 {
            return self.cancel_jog().await;
        }

        let jog = ContinuousJog {
            x_direction,
            y_direction,
            axis: None,
            feed_rate,
        };
        let mut session = self.session.lock().await;
        session.continuous_jog = Some(jog);
        if let Err(error) = Self::send_jog_chunk(&mut session, jog).await {
            session.continuous_jog = None;
            return Err(error);
        }
        Ok(())
    }

    async fn cancel_jog(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        session.continuous_jog = None;
        Self::send_realtime(&mut session, 0x85).await
    }

    async fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::send_command(&mut session, command).await
    }

    async fn load_program(&self, path: &Path) -> CncResult<()> {
        let content = fs::read_to_string(path)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        let lines = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("program.nc")
            .to_owned();

        let mut session = self.session.lock().await;
        session.program = lines;
        session.program_name = Some(name);
        session.current_line = 0;
        Ok(())
    }

    async fn program_run(&self, start_line: u32) -> CncResult<()> {
        let mut session = self.session.lock().await;
        let start = start_line as usize;
        if start > session.program.len() {
            return Err(ControllerError::new(
                "bad_program",
                "start line is outside the loaded GRBL program",
            ));
        }

        for index in start..session.program.len() {
            let line = session.program[index].clone();
            Self::send_command(&mut session, &line).await?;
            session.current_line = (index + 1) as u32;
        }
        Ok(())
    }

    async fn program_pause(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::send_realtime(&mut session, b'!').await
    }

    async fn program_resume(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::send_realtime(&mut session, b'~').await
    }

    async fn program_abort(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::send_realtime(&mut session, 0x18).await?;
        session.current_line = 0;
        Ok(())
    }

    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        if !command.rpm.is_finite() || command.rpm < 0.0 {
            return Err(ControllerError::new(
                "bad_spindle",
                "spindle RPM must be finite and non-negative",
            ));
        }

        let mut session = self.session.lock().await;
        let gcode = match command.direction {
            SpindleDirection::Stopped => "M5".to_owned(),
            SpindleDirection::Clockwise => format!("M3 S{:.0}", command.rpm),
            SpindleDirection::CounterClockwise => format!("M4 S{:.0}", command.rpm),
        };
        Self::send_command(&mut session, &gcode).await?;
        session.spindle_command = command;
        Ok(())
    }

    async fn clear_error(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::send_command(&mut session, "$X").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spindle_command() -> SpindleCommand {
        SpindleCommand {
            direction: SpindleDirection::Clockwise,
            rpm: 1200.0,
        }
    }

    #[test]
    fn extracts_complete_line_and_preserves_following_buffered_lines() {
        let mut buffer = b"ok\n<Idle|MPos:1,2,3|FS:0,0>\n".to_vec();

        assert_eq!(
            GrblAdapter::extract_line(&mut buffer).unwrap().as_deref(),
            Some("ok")
        );
        assert_eq!(
            GrblAdapter::extract_line(&mut buffer).unwrap().as_deref(),
            Some("<Idle|MPos:1,2,3|FS:0,0>")
        );
        assert!(buffer.is_empty());
    }

    #[test]
    fn rejects_oversized_line_even_when_terminator_is_already_buffered() {
        let mut buffer = vec![b'x'; MAX_RESPONSE_LINE + 1];
        buffer.push(b'\n');

        let error = GrblAdapter::extract_line(&mut buffer).unwrap_err();
        assert_eq!(error.code, "protocol");
    }

    #[test]
    fn parses_machine_position_and_derives_work_offset() {
        let status = GrblAdapter::parse_status(
            "<Idle|MPos:10.000,20.000,30.000|WPos:1.000,2.000,3.000|FS:100.0,0.0>",
            None,
            CoordinateSystem::G55,
            spindle_command(),
            Some("part.ngc"),
            2,
            5,
        )
        .unwrap();

        assert_eq!(status.position.x, 10.0);
        assert_eq!(status.position.y, 20.0);
        assert_eq!(status.position.z, 30.0);
        assert_eq!(status.work_offset.x, 9.0);
        assert_eq!(status.work_offset.y, 18.0);
        assert_eq!(status.work_offset.z, 27.0);
        assert_eq!(status.active_coordinate_system, CoordinateSystem::G55);
        assert_eq!(status.loaded_program.as_deref(), Some("part.ngc"));
        assert_eq!((status.current_line, status.total_lines), (2, 5));
    }

    #[test]
    fn reuses_cached_work_offset_when_grbl_omits_wco() {
        let status = GrblAdapter::parse_status(
            "<Idle|WPos:1.000,2.000,3.000|FS:0,0>",
            Some([9.0, 18.0, 27.0]),
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap();

        assert_eq!(status.position.x, 10.0);
        assert_eq!(status.position.y, 20.0);
        assert_eq!(status.position.z, 30.0);
        assert_eq!(status.work_offset.x, 9.0);
        assert_eq!(status.work_offset.y, 18.0);
        assert_eq!(status.work_offset.z, 27.0);
    }

    #[test]
    fn reported_work_coordinate_offset_takes_precedence() {
        let status = GrblAdapter::parse_status(
            "<Idle|MPos:10,20,30|WPos:1,2,3|WCO:8,17,26|FS:0,0>",
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap();

        assert_eq!(status.work_offset.x, 8.0);
        assert_eq!(status.work_offset.y, 17.0);
        assert_eq!(status.work_offset.z, 26.0);
    }

    #[test]
    fn rejects_rotary_axes_for_grbl_jog() {
        for axis in [Axis::A, Axis::B] {
            let error = GrblAdapter::ensure_supported_axis(axis).unwrap_err();
            assert_eq!(error.code, "unsupported_axis");
        }

        for axis in [Axis::X, Axis::Y, Axis::Z] {
            assert!(GrblAdapter::ensure_supported_axis(axis).is_ok());
        }
    }

    #[test]
    fn rejects_malformed_reported_coordinate_fields() {
        for response in [
            "<Idle|MPos:1,NaN,3|FS:0,0>",
            "<Idle|WPos:1,2|FS:0,0>",
            "<Idle|WCO:1,2,Infinity|FS:0,0>",
        ] {
            assert!(
                GrblAdapter::parse_status(
                    response,
                    None,
                    CoordinateSystem::G54,
                    spindle_command(),
                    None,
                    0,
                    0,
                )
                .is_err(),
                "expected malformed status field to be rejected: {response}"
            );
        }
    }

    #[test]
    fn rejects_malformed_status_and_non_finite_coordinates() {
        assert!(GrblAdapter::parse_status(
            "Idle|MPos:1,2,3",
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .is_err());

        assert!(GrblAdapter::parse_triplet("1,NaN,3").is_none());
        assert!(GrblAdapter::parse_triplet("1,2").is_none());
    }
}
