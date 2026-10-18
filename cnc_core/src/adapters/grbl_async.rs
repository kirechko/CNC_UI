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
const MAX_COMMAND_LENGTH: usize = 80;
const MAX_PROGRAM_BYTES: usize = 5 * 1024 * 1024;
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
    work_offsets: HashMap<CoordinateSystem, [f64; 3]>,
    protocol_poisoned: bool,
    program_streaming: bool,
    jog_chunk_active: bool,
    program: Vec<String>,
    program_name: Option<String>,
    current_line: u32,
    active_coordinate_system: CoordinateSystem,
    spindle_command: SpindleCommand,
    min_spindle_rpm: Option<f64>,
    max_spindle_rpm: Option<f64>,
    continuous_jog: Option<ContinuousJog>,
}

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
                min_spindle_rpm: None,
                max_spindle_rpm: None,
                continuous_jog: None,
                rx_buffer: Vec::new(),
                work_offsets: HashMap::new(),
                protocol_poisoned: false,
                program_streaming: false,
                jog_chunk_active: false,
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
            let line = std::str::from_utf8(&line[..line.len() - 1]).map_err(|error| {
                ControllerError::new(
                    "protocol",
                    format!("GRBL response is not valid UTF-8: {error}"),
                )
            })?;
            return Ok(Some(line.trim().to_string()));
        }

        if rx_buffer.len() > MAX_RESPONSE_LINE {
            return Err(ControllerError::new(
                "protocol",
                "GRBL response line exceeds size limit",
            ));
        }

        Ok(None)
    }

    async fn read_line(
        port: &mut SerialStream,
        rx_buffer: &mut Vec<u8>,
        read_timeout: Duration,
    ) -> CncResult<String> {
        loop {
            if let Some(line) = Self::extract_line(rx_buffer)? {
                return Ok(line);
            }

            let mut chunk = [0_u8; 256];
            let bytes_read = timeout(read_timeout, port.read(&mut chunk))
                .await
                .map_err(|_| ControllerError::new("timeout", "timed out waiting for GRBL response"))?
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

    fn ensure_protocol_usable(protocol_poisoned: bool) -> CncResult<()> {
        if protocol_poisoned {
            return Err(ControllerError::new(
                "protocol_desynchronized",
                "GRBL response timed out; reconnect before sending more commands",
            ));
        }
        Ok(())
    }

    fn validate_command(command: &str) -> CncResult<()> {
        if command.contains(['\n', '\r']) {
            return Err(ControllerError::new(
                "bad_command",
                "a GRBL command must contain exactly one line",
            ));
        }
        if !command.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) {
            return Err(ControllerError::new(
                "bad_command",
                "GRBL commands must contain printable ASCII characters only",
            ));
        }

        let command = command.trim();
        if command.len() > MAX_COMMAND_LENGTH {
            return Err(ControllerError::new(
                "bad_command",
                format!("GRBL command exceeds the {MAX_COMMAND_LENGTH}-byte line limit"),
            ));
        }

        Ok(())
    }

    async fn send_command(session: &mut Session, command: &str) -> CncResult<()> {
        Self::ensure_protocol_usable(session.protocol_poisoned)?;
        Self::validate_command(command)?;

        if command.trim().is_empty() {
            return Ok(());
        }

        if let Err(error) = session.port.write_all(command.trim().as_bytes()).await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }
        if let Err(error) = session.port.write_all(b"\n").await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }

        match timeout(RESPONSE_TIMEOUT, async {
            loop {
                let line = match Self::read_line(
                    &mut session.port,
                    &mut session.rx_buffer,
                    RESPONSE_TIMEOUT,
                )
                .await
                {
                    Ok(line) => line,
                    Err(error) => {
                        session.protocol_poisoned = true;
                        return Err(error);
                    }
                };
                if line == "ok" {
                    if let Some(system) = Self::coordinate_system_select(command) {
                        session.active_coordinate_system = system;
                    }
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
        {
            Ok(result) => result,
            Err(_) => {
                session.protocol_poisoned = true;
                Err(ControllerError::new(
                    "timeout",
                    "timed out waiting for GRBL acknowledgement; reconnect required",
                ))
            }
        }
    }

    async fn send_realtime(session: &mut Session, byte: u8) -> CncResult<()> {
        // Realtime-команды не ждут ACK; блокировать отмену движения из-за
        // рассинхронизации обычного протокола небезопасно.
        if let Err(error) = session.port.write_all(&[byte]).await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }
        Ok(())
    }

    fn format_axis_jog_command(axis: Axis, distance: f64, feed_rate: f64) -> CncResult<String> {
        if !distance.is_finite() || distance == 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog distance must be finite and non-zero",
            ));
        }
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be finite and positive",
            ));
        }

        let command = format!(
            "$J=G21 G91 {}{distance} F{feed_rate}",
            Self::axis_letter(axis)
        );
        Self::validate_command(&command)?;
        Ok(command)
    }

    fn format_xy_jog_command(
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<String> {
        if ![-1, 0, 1].contains(&x_direction)
            || ![-1, 0, 1].contains(&y_direction)
            || (x_direction == 0 && y_direction == 0)
            || !feed_rate.is_finite()
            || feed_rate <= 0.0
        {
            return Err(ControllerError::new("bad_jog", "invalid continuous XY jog"));
        }

        let norm = f64::from(x_direction * x_direction + y_direction * y_direction).sqrt();
        let x = JOG_CHUNK_MM * f64::from(x_direction) / norm;
        let y = JOG_CHUNK_MM * f64::from(y_direction) / norm;
        let command = format!("$J=G21 G91 X{x:.4} Y{y:.4} F{feed_rate}");
        Self::validate_command(&command)?;
        Ok(command)
    }

    async fn send_jog_chunk(session: &mut Session, jog: ContinuousJog) -> CncResult<()> {
        let command = if let Some(axis) = jog.axis {
            Self::format_axis_jog_command(
                axis,
                JOG_CHUNK_MM * f64::from(jog.x_direction),
                jog.feed_rate,
            )?
        } else {
            Self::format_xy_jog_command(jog.x_direction, jog.y_direction, jog.feed_rate)?
        };

        Self::send_command(session, &command).await
    }

    async fn stream_next_program_line(session: &mut Session) -> CncResult<()> {
        if !session.program_streaming {
            return Ok(());
        }

        let index = session.current_line as usize;
        if index >= session.program.len() {
            session.program_streaming = false;
            return Ok(());
        }

        let line = session.program[index].clone();
        if let Err(error) = Self::send_command(session, &line).await {
            session.program_streaming = false;
            return Err(error);
        }

        session.current_line = (index + 1) as u32;
        // Оставляем streaming активным до следующего Idle-ответа:
        // подтверждение GRBL означает приём строки, а не завершение движения.
        Ok(())
    }

    async fn query_status(session: &mut Session) -> CncResult<String> {
        Self::ensure_protocol_usable(session.protocol_poisoned)?;
        if let Err(error) = session.port.write_all(b"?").await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }

        match timeout(STATUS_TIMEOUT, async {
            loop {
                let line = match Self::read_line(
                    &mut session.port,
                    &mut session.rx_buffer,
                    STATUS_TIMEOUT,
                )
                .await
                {
                    Ok(line) => line,
                    Err(error) => {
                        session.protocol_poisoned = true;
                        return Err(error);
                    }
                };
                if line.starts_with('<') && line.ends_with('>') {
                    return Ok(line);
                }
            }
        })
        .await
        {
            Ok(result) => result,
            // Потеря ответа на realtime-запрос `?` не рассинхронизирует очередь ACK
            // команд. Не блокируем дальнейшие запросы и команды из-за единичного тайм-аута.
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for GRBL status",
            )),
        }
    }

    async fn query_settings(session: &mut Session) -> CncResult<HashMap<u16, f64>> {
        Self::ensure_protocol_usable(session.protocol_poisoned)?;
        if let Err(error) = session.port.write_all(b"$$\n").await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }

        match timeout(RESPONSE_TIMEOUT, async {
            let mut settings = HashMap::new();

            loop {
                let line = match Self::read_line(
                    &mut session.port,
                    &mut session.rx_buffer,
                    RESPONSE_TIMEOUT,
                )
                .await
                {
                    Ok(line) => line,
                    Err(error) => {
                        session.protocol_poisoned = true;
                        return Err(error);
                    }
                };
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
        {
            Ok(result) => result,
            Err(_) => {
                session.protocol_poisoned = true;
                Err(ControllerError::new(
                    "timeout",
                    "timed out waiting for GRBL settings; reconnect required",
                ))
            }
        }
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

    fn parse_feed_spindle(value: &str) -> CncResult<(f64, f64)> {
        let mut values = value.split(',');
        let feed_rate = values
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| ControllerError::new("protocol", "invalid GRBL feed-rate field"))?;
        let spindle_rpm = values
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| ControllerError::new("protocol", "invalid GRBL spindle-speed field"))?;

        if values.next().is_some() {
            return Err(ControllerError::new(
                "protocol",
                "GRBL FS field must contain exactly two values",
            ));
        }

        Ok((feed_rate, spindle_rpm))
    }

    fn is_known_machine_state(state: &str) -> bool {
        let (name, detail) = match state.split_once(':') {
            Some((name, detail)) => (name.to_ascii_lowercase(), Some(detail)),
            None => (state.to_ascii_lowercase(), None),
        };

        match name.as_str() {
            "idle" | "run" | "jog" | "check" | "home" | "sleep" => detail.is_none(),
            "hold" => matches!(detail, Some("0" | "1")),
            "door" => detail
                .and_then(|value| value.parse::<u8>().ok())
                .is_some_and(|state| state <= 3),
            "alarm" => match detail {
                None => true,
                Some(value) => value.parse::<u16>().is_ok_and(|code| code > 0),
            },
            _ => false,
        }
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
        let state_lower = state.to_ascii_lowercase();
        if !Self::is_known_machine_state(state) {
            return Err(ControllerError::new(
                "protocol",
                format!("unknown GRBL machine state: {state}"),
            ));
        }

        let mut machine_position = None;
        let mut work_position = None;
        let mut work_offset = None;
        let mut feed_rate = 0.0;
        let mut actual_rpm = 0.0;
        let mut feed_spindle_seen = false;
        let mut feed_seen = false;

        for field in fields {
            if let Some(value) = field.strip_prefix("MPos:") {
                if machine_position.is_some() {
                    return Err(ControllerError::new(
                        "protocol",
                        "duplicate GRBL MPos field",
                    ));
                }
                machine_position =
                    Some(Self::parse_triplet(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL MPos field")
                    })?);
            } else if let Some(value) = field.strip_prefix("WPos:") {
                if work_position.is_some() {
                    return Err(ControllerError::new(
                        "protocol",
                        "duplicate GRBL WPos field",
                    ));
                }
                work_position =
                    Some(Self::parse_triplet(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL WPos field")
                    })?);
            } else if let Some(value) = field.strip_prefix("WCO:") {
                if work_offset.is_some() {
                    return Err(ControllerError::new("protocol", "duplicate GRBL WCO field"));
                }
                work_offset =
                    Some(Self::parse_triplet(value).ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL WCO field")
                    })?);
            } else if let Some(value) = field.strip_prefix("FS:") {
                if feed_seen {
                    return Err(ControllerError::new(
                        "protocol",
                        "duplicate GRBL feed-rate field",
                    ));
                }
                (feed_rate, actual_rpm) = Self::parse_feed_spindle(value)?;
                feed_spindle_seen = true;
                feed_seen = true;
            } else if let Some(value) = field.strip_prefix("F:") {
                if feed_seen || feed_spindle_seen {
                    return Err(ControllerError::new(
                        "protocol",
                        "duplicate GRBL feed-rate field",
                    ));
                }
                feed_rate = value
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && *value >= 0.0)
                    .ok_or_else(|| {
                        ControllerError::new("protocol", "invalid GRBL feed-rate field")
                    })?;
                feed_seen = true;
            }
        }

        if machine_position.is_none() && work_position.is_none() {
            return Err(ControllerError::new(
                "protocol",
                "GRBL status response contains no machine or work position",
            ));
        }
        if machine_position.is_none() && work_offset.is_none() && cached_work_offset.is_none() {
            return Err(ControllerError::new(
                "protocol",
                "cannot determine work coordinates without a work coordinate offset",
            ));
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
        if offset.iter().any(|value| !value.is_finite()) {
            return Err(ControllerError::new(
                "protocol",
                "GRBL status produced a non-finite work coordinate offset",
            ));
        }

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
        if machine.iter().any(|value| !value.is_finite()) {
            return Err(ControllerError::new(
                "protocol",
                "GRBL status produced a non-finite machine position",
            ));
        }

        let alarm = state_lower.starts_with("alarm");
        let running = state_lower.starts_with("run");
        let jogging = state_lower.starts_with("jog");
        // GRBL reports the door interlock as Door:n; surface it as paused
        // instead of presenting an interlocked machine as idle.
        let paused = state_lower.starts_with("hold") || state_lower.starts_with("door");
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

    fn validate_spindle_command(
        command: SpindleCommand,
        min_rpm: Option<f64>,
        max_rpm: Option<f64>,
    ) -> CncResult<()> {
        if command.direction == SpindleDirection::Stopped {
            return Ok(());
        }

        if min_rpm.is_some_and(|minimum| command.rpm < minimum)
            || max_rpm.is_some_and(|maximum| command.rpm > maximum)
        {
            return Err(ControllerError::new(
                "spindle_limit",
                format!(
                    "requested {:.0} RPM is outside the configured range {:.0}–{:.0} RPM",
                    command.rpm,
                    min_rpm.unwrap_or(0.0),
                    max_rpm.unwrap_or(f64::INFINITY),
                ),
            ));
        }

        Ok(())
    }

    fn normalize_spindle_command(command: SpindleCommand) -> SpindleCommand {
        if command.rpm == 0.0 || command.direction == SpindleDirection::Stopped {
            SpindleCommand {
                direction: SpindleDirection::Stopped,
                rpm: 0.0,
            }
        } else {
            command
        }
    }

    fn validate_continuous_xy_jog(
        x_direction: i8,
        y_direction: i8,
        feed_rate: f64,
    ) -> CncResult<()> {
        if ![-1, 0, 1].contains(&x_direction) || ![-1, 0, 1].contains(&y_direction) {
            return Err(ControllerError::new("bad_jog", "invalid continuous XY jog"));
        }

        // Скорость не нужна для остановки, когда обе компоненты равны нулю.
        if x_direction == 0 && y_direction == 0 {
            return Ok(());
        }

        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }

        Ok(())
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

    fn ensure_supported_coordinate_system(system: CoordinateSystem) -> CncResult<()> {
        match system {
            CoordinateSystem::G54
            | CoordinateSystem::G55
            | CoordinateSystem::G56
            | CoordinateSystem::G57
            | CoordinateSystem::G58
            | CoordinateSystem::G59 => Ok(()),
            CoordinateSystem::G59_1 | CoordinateSystem::G59_2 | CoordinateSystem::G59_3 => {
                Err(ControllerError::new(
                    "unsupported_coordinate_system",
                    format!("GRBL 1.1 does not support {}", system.label()),
                ))
            }
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

    fn ensure_manual_command_allowed(program_streaming: bool) -> CncResult<()> {
        if program_streaming {
            return Err(ControllerError::new(
                "motion_busy",
                "cannot issue a manual command while a program is streaming",
            ));
        }
        Ok(())
    }

    fn is_idle_status(status_response: &str) -> bool {
        status_response
            .strip_prefix('<')
            .and_then(|response| response.strip_suffix('>'))
            .and_then(|response| response.split('|').next())
            .is_some_and(|state| state.eq_ignore_ascii_case("Idle"))
    }

    fn ensure_controller_idle(status_response: &str) -> CncResult<()> {
        let state = status_response
            .strip_prefix('<')
            .and_then(|response| response.strip_suffix('>'))
            .and_then(|response| response.split('|').next())
            .ok_or_else(|| ControllerError::new("protocol", "malformed GRBL status response"))?;

        if !Self::is_known_machine_state(state) {
            return Err(ControllerError::new(
                "protocol",
                format!("unknown GRBL machine state: {state}"),
            ));
        }
        if !state.eq_ignore_ascii_case("Idle") {
            return Err(ControllerError::new(
                "motion_busy",
                format!("cannot start a program while GRBL state is {state}"),
            ));
        }

        Ok(())
    }

    fn ensure_jog_idle(jog_active: bool) -> CncResult<()> {
        if jog_active {
            return Err(ControllerError::new(
                "motion_busy",
                "cannot issue this command while a continuous Jog is active",
            ));
        }
        Ok(())
    }

    fn ensure_program_idle(program_streaming: bool, jog_active: bool) -> CncResult<()> {
        if program_streaming || jog_active {
            return Err(ControllerError::new(
                "motion_busy",
                "cannot load or start a program while another program or Jog is active",
            ));
        }
        Ok(())
    }

    fn parse_program_lines(content: &str) -> CncResult<Vec<String>> {
        if content.len() > MAX_PROGRAM_BYTES {
            return Err(ControllerError::new(
                "bad_program",
                format!("GRBL program exceeds the {MAX_PROGRAM_BYTES}-byte size limit"),
            ));
        }

        let mut lines = Vec::new();

        for (line_index, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            Self::validate_command(line).map_err(|error| {
                ControllerError::new(
                    "bad_program",
                    format!(
                        "invalid GRBL program line {}: {}",
                        line_index + 1,
                        error.message
                    ),
                )
            })?;
            lines.push(line.to_owned());
        }

        Ok(lines)
    }

    fn coordinate_system_select(command: &str) -> Option<CoordinateSystem> {
        let mut selected = None;
        let mut in_parenthesis_comment = false;

        for line in command.lines() {
            for (index, byte) in line.bytes().enumerate() {
                match byte {
                    b'(' => {
                        in_parenthesis_comment = true;
                        continue;
                    }
                    b')' => {
                        in_parenthesis_comment = false;
                        continue;
                    }
                    b';' if !in_parenthesis_comment => break,
                    b'G' | b'g' if !in_parenthesis_comment => {
                        let bytes = line.as_bytes();
                        let mut end = index + 1;
                        while end < bytes.len()
                            && (bytes[end].is_ascii_digit() || bytes[end] == b'.')
                        {
                            end += 1;
                        }

                        if end == index + 1 {
                            continue;
                        }

                        let code = line[index + 1..end].parse::<f64>().ok()?;
                        let system = match code {
                            54.0 => CoordinateSystem::G54,
                            55.0 => CoordinateSystem::G55,
                            56.0 => CoordinateSystem::G56,
                            57.0 => CoordinateSystem::G57,
                            58.0 => CoordinateSystem::G58,
                            59.0 => CoordinateSystem::G59,
                            59.1 => CoordinateSystem::G59_1,
                            59.2 => CoordinateSystem::G59_2,
                            59.3 => CoordinateSystem::G59_3,
                            _ => continue,
                        };
                        selected = Some(system);
                    }
                    _ => {}
                }
            }
        }

        selected
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

        if Self::is_idle_status(&response) {
            session.jog_chunk_active = false;
            if session.program_streaming {
                Self::stream_next_program_line(&mut session).await?;
            } else if let Some(jog) = session.continuous_jog {
                if let Err(error) = Self::send_jog_chunk(&mut session, jog).await {
                    session.continuous_jog = None;
                    return Err(error);
                }
                session.jog_chunk_active = true;
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
            spindle: SpindleConfiguration { min_rpm, max_rpm },
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
        let command = format!("G10 L20 P{} X0 Y0 Z0", Self::coordinate_number(system));
        Self::send_command(&mut session, &command).await?;
        // G10 L20 Pn обновляет смещение указанной системы, но не выбирает её.
        // Активную систему отражает только select_coordinate_system().
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
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        let chunk_active = session.jog_chunk_active;
        session.continuous_jog = Some(jog);

        if chunk_active {
            Self::send_realtime(&mut session, 0x85).await?;
            session.jog_chunk_active = false;
            return Ok(());
        }

        if let Err(error) = Self::send_jog_chunk(&mut session, jog).await {
            session.continuous_jog = None;
            return Err(error);
        }
        session.jog_chunk_active = true;
        Ok(())
    }

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

        let jog = ContinuousJog {
            x_direction,
            y_direction,
            axis: None,
            feed_rate,
        };
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        let chunk_active = session.jog_chunk_active;
        session.continuous_jog = Some(jog);

        if chunk_active {
            Self::send_realtime(&mut session, 0x85).await?;
            session.jog_chunk_active = false;
            return Ok(());
        }

        if let Err(error) = Self::send_jog_chunk(&mut session, jog).await {
            session.continuous_jog = None;
            return Err(error);
        }
        session.jog_chunk_active = true;
        Ok(())
    }

    async fn cancel_jog(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        session.continuous_jog = None;
        session.jog_chunk_active = false;
        Self::send_realtime(&mut session, 0x85).await
    }

    async fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
        Self::send_command(&mut session, command).await
    }

    async fn load_program(&self, path: &Path) -> CncResult<()> {
        let file = fs::File::open(path)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        let mut bytes = Vec::with_capacity(MAX_PROGRAM_BYTES.min(64 * 1024));
        file.take((MAX_PROGRAM_BYTES as u64) + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;

        if bytes.len() > MAX_PROGRAM_BYTES {
            return Err(ControllerError::new(
                "bad_program",
                format!("GRBL program exceeds the {MAX_PROGRAM_BYTES}-byte size limit"),
            ));
        }

        let content = String::from_utf8(bytes)
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        let lines = Self::parse_program_lines(&content)?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("program.nc")
            .to_owned();

        let mut session = self.session.lock().await;
        Self::ensure_program_idle(
            session.program_streaming,
            session.continuous_jog.is_some() || session.jog_chunk_active,
        )?;
        session.program = lines;
        session.program_name = Some(name);
        session.current_line = 0;
        session.program_streaming = false;
        Ok(())
    }

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
        session.program_streaming = false;
        session.continuous_jog = None;
        session.jog_chunk_active = false;
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

    async fn clear_error(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        Self::ensure_manual_command_allowed(session.program_streaming)?;
        Self::ensure_jog_idle(session.continuous_jog.is_some() || session.jog_chunk_active)?;
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
    fn detects_coordinate_system_changes_in_gcode_without_comment_matches() {
        assert_eq!(
            GrblAdapter::coordinate_system_select("G55 G0 X1"),
            Some(CoordinateSystem::G55)
        );
        assert_eq!(
            GrblAdapter::coordinate_system_select("G54 G1X1 G59.3"),
            Some(CoordinateSystem::G59_3)
        );
        assert_eq!(
            GrblAdapter::coordinate_system_select("G1 X1 (G55) ; G56"),
            None
        );
        assert_eq!(GrblAdapter::coordinate_system_select("G54.1 P1"), None);
    }

    #[test]
    fn stopped_or_zero_speed_spindle_commands_are_normalized() {
        let stopped = GrblAdapter::normalize_spindle_command(SpindleCommand {
            direction: SpindleDirection::Clockwise,
            rpm: 0.0,
        });
        assert_eq!(stopped.direction, SpindleDirection::Stopped);
        assert_eq!(stopped.rpm, 0.0);

        let zero_direction = GrblAdapter::normalize_spindle_command(SpindleCommand {
            direction: SpindleDirection::Stopped,
            rpm: 1200.0,
        });
        assert_eq!(zero_direction.direction, SpindleDirection::Stopped);
        assert_eq!(zero_direction.rpm, 0.0);

        let running = spindle_command();
        assert_eq!(GrblAdapter::normalize_spindle_command(running), running);
    }

    #[test]
    fn spindle_commands_respect_reported_controller_limits() {
        let command = SpindleCommand {
            direction: SpindleDirection::Clockwise,
            rpm: 1200.0,
        };

        assert!(GrblAdapter::validate_spindle_command(command, Some(500.0), Some(24000.0)).is_ok());
        assert_eq!(
            GrblAdapter::validate_spindle_command(command, Some(1500.0), Some(24000.0))
                .unwrap_err()
                .code,
            "spindle_limit"
        );
        assert_eq!(
            GrblAdapter::validate_spindle_command(command, Some(500.0), Some(1000.0))
                .unwrap_err()
                .code,
            "spindle_limit"
        );

        let stopped = SpindleCommand {
            direction: SpindleDirection::Stopped,
            rpm: 0.0,
        };
        assert!(GrblAdapter::validate_spindle_command(stopped, Some(500.0), Some(24000.0)).is_ok());
    }

    #[test]
    fn axis_jog_command_preserves_metric_units_and_numeric_precision() {
        let command = GrblAdapter::format_axis_jog_command(Axis::X, 0.00001, 0.25).unwrap();

        assert_eq!(command, "$J=G21 G91 X0.00001 F0.25");
    }

    #[test]
    fn xy_jog_command_explicitly_uses_metric_units() {
        let command = GrblAdapter::format_xy_jog_command(1, -1, 1200.0).unwrap();

        assert_eq!(command, "$J=G21 G91 X7.0711 Y-7.0711 F1200");
        assert!(GrblAdapter::format_xy_jog_command(0, 0, 1200.0).is_err());
    }

    #[test]
    fn axis_jog_command_rejects_zero_or_non_finite_values() {
        assert!(GrblAdapter::format_axis_jog_command(Axis::X, 0.0, 100.0).is_err());
        assert!(GrblAdapter::format_axis_jog_command(Axis::X, f64::NAN, 100.0).is_err());
        assert!(GrblAdapter::format_axis_jog_command(Axis::X, 1.0, 0.0).is_err());
        assert!(GrblAdapter::format_axis_jog_command(Axis::X, 1.0, f64::INFINITY).is_err());
    }

    #[test]
    fn rejects_non_ascii_and_control_characters_in_commands() {
        for command in ["G1 X1\tY2", "G1 X1\0", "G1 X1 °"] {
            let error = GrblAdapter::validate_command(command).unwrap_err();
            assert_eq!(error.code, "bad_command", "command: {command:?}");
        }
    }

    #[test]
    fn prevents_manual_commands_during_program_streaming() {
        assert!(GrblAdapter::ensure_manual_command_allowed(false).is_ok());

        let error = GrblAdapter::ensure_manual_command_allowed(true).unwrap_err();
        assert_eq!(error.code, "motion_busy");
    }

    #[test]
    fn idle_detection_requires_an_exact_valid_state_name() {
        assert!(GrblAdapter::is_idle_status("<Idle|MPos:0,0,0>"));
        assert!(GrblAdapter::is_idle_status("<idle|MPos:0,0,0>"));
        assert!(!GrblAdapter::is_idle_status("<Idleish|MPos:0,0,0>"));
        assert!(!GrblAdapter::is_idle_status("<Run|MPos:0,0,0>"));
        assert!(!GrblAdapter::is_idle_status("Idle|MPos:0,0,0"));
    }

    #[test]
    fn only_starts_program_when_grbl_reports_idle() {
        assert!(GrblAdapter::ensure_controller_idle("<Idle|MPos:0,0,0>").is_ok());

        for state in ["Run", "Jog", "Hold:0", "Door:1", "Alarm:1"] {
            let response = format!("<{state}|MPos:0,0,0>");
            let error = GrblAdapter::ensure_controller_idle(&response).unwrap_err();
            assert_eq!(error.code, "motion_busy", "state: {state}");
        }

        assert_eq!(
            GrblAdapter::ensure_controller_idle("<Unknown|MPos:0,0,0>")
                .unwrap_err()
                .code,
            "protocol"
        );
    }

    #[test]
    fn prevents_conflicting_commands_during_continuous_jog() {
        assert!(GrblAdapter::ensure_jog_idle(false).is_ok());

        let error = GrblAdapter::ensure_jog_idle(true).unwrap_err();
        assert_eq!(error.code, "motion_busy");
    }

    #[test]
    fn prevents_program_load_or_start_during_program_or_jog_motion() {
        assert!(GrblAdapter::ensure_program_idle(false, false).is_ok());

        for (program_streaming, jog_active) in [(true, false), (false, true), (true, true)] {
            let error =
                GrblAdapter::ensure_program_idle(program_streaming, jog_active).unwrap_err();
            assert_eq!(error.code, "motion_busy");
        }
    }

    #[test]
    fn rejects_programs_over_the_size_limit() {
        let content = " ".repeat(MAX_PROGRAM_BYTES + 1);
        let error = GrblAdapter::parse_program_lines(&content).unwrap_err();

        assert_eq!(error.code, "bad_program");
        assert!(error.message.contains("size limit"));
    }

    #[test]
    fn validates_every_program_line_before_execution() {
        let lines = GrblAdapter::parse_program_lines("\nG0 X1\n\nG1 X2\n").unwrap();
        assert_eq!(lines, ["G0 X1", "G1 X2"]);

        let error = GrblAdapter::parse_program_lines(&format!(
            "G0 X1\n{}\nG1 X2",
            "G".repeat(MAX_COMMAND_LENGTH + 1)
        ))
        .unwrap_err();

        assert_eq!(error.code, "bad_program");
        assert!(error.message.contains("line 2"));
    }

    #[test]
    fn validates_grbl_command_line_length_and_single_line_format() {
        assert!(GrblAdapter::validate_command("G1 X10 Y20").is_ok());
        assert!(GrblAdapter::validate_command(&"G".repeat(MAX_COMMAND_LENGTH)).is_ok());

        let error = GrblAdapter::validate_command(&"G".repeat(MAX_COMMAND_LENGTH + 1)).unwrap_err();
        assert_eq!(error.code, "bad_command");

        let error = GrblAdapter::validate_command("G1 X1\nM3 S1000").unwrap_err();
        assert_eq!(error.code, "bad_command");
    }

    #[test]
    fn rejects_operations_after_protocol_timeout() {
        let error = GrblAdapter::ensure_protocol_usable(true).unwrap_err();
        assert_eq!(error.code, "protocol_desynchronized");
        assert!(GrblAdapter::ensure_protocol_usable(false).is_ok());
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
    fn rejects_non_utf8_response_lines() {
        let mut buffer = vec![0xff, b'\n'];

        let error = GrblAdapter::extract_line(&mut buffer).unwrap_err();
        assert_eq!(error.code, "protocol");
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
    fn continuous_xy_stop_does_not_require_a_positive_feed_rate() {
        assert!(GrblAdapter::validate_continuous_xy_jog(0, 0, 0.0).is_ok());
        assert!(GrblAdapter::validate_continuous_xy_jog(0, 0, f64::NAN).is_ok());
        assert!(GrblAdapter::validate_continuous_xy_jog(1, 0, 0.0).is_err());
        assert!(GrblAdapter::validate_continuous_xy_jog(2, 0, 100.0).is_err());
    }

    #[test]
    fn rejects_coordinate_systems_not_supported_by_grbl() {
        for system in [
            CoordinateSystem::G59_1,
            CoordinateSystem::G59_2,
            CoordinateSystem::G59_3,
        ] {
            let error = GrblAdapter::ensure_supported_coordinate_system(system).unwrap_err();
            assert_eq!(error.code, "unsupported_coordinate_system");
        }

        for system in [
            CoordinateSystem::G54,
            CoordinateSystem::G55,
            CoordinateSystem::G56,
            CoordinateSystem::G57,
            CoordinateSystem::G58,
            CoordinateSystem::G59,
        ] {
            assert!(GrblAdapter::ensure_supported_coordinate_system(system).is_ok());
        }
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
    fn parses_feed_only_status_reports() {
        let status = GrblAdapter::parse_status(
            "<Idle|MPos:1,2,3|F:120.5>",
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap();

        assert_eq!(status.feed_rate, 120.5);
        assert_eq!(status.spindle.actual_rpm, 0.0);
    }

    #[test]
    fn rejects_duplicate_feed_telemetry_fields() {
        for response in [
            "<Idle|MPos:0,0,0|F:100|F:200>",
            "<Idle|MPos:0,0,0|F:100|FS:200,300>",
            "<Idle|MPos:0,0,0|FS:100,300|F:200>",
        ] {
            let error = GrblAdapter::parse_status(
                response,
                None,
                CoordinateSystem::G54,
                spindle_command(),
                None,
                0,
                0,
            )
            .unwrap_err();

            assert_eq!(error.code, "protocol", "response: {response}");
        }
    }

    #[test]
    fn rejects_malformed_feed_and_spindle_telemetry() {
        for response in [
            "<Idle|MPos:0,0,0|FS:bad,0>",
            "<Idle|MPos:0,0,0|FS:100,NaN>",
            "<Idle|MPos:0,0,0|FS:-1,0>",
            "<Idle|MPos:0,0,0|FS:100,0,extra>",
        ] {
            let error = GrblAdapter::parse_status(
                response,
                None,
                CoordinateSystem::G54,
                spindle_command(),
                None,
                0,
                0,
            )
            .unwrap_err();

            assert_eq!(error.code, "protocol", "response: {response}");
        }
    }

    #[test]
    fn rejects_duplicate_position_and_telemetry_fields() {
        for response in [
            "<Idle|MPos:1,2,3|MPos:4,5,6|WCO:0,0,0|FS:0,0>",
            "<Idle|WPos:1,2,3|WPos:4,5,6|WCO:0,0,0|FS:0,0>",
            "<Idle|MPos:1,2,3|WCO:0,0,0|WCO:1,1,1|FS:0,0>",
            "<Idle|MPos:1,2,3|WCO:0,0,0|FS:0,0|FS:1,1>",
        ] {
            let error = GrblAdapter::parse_status(
                response,
                None,
                CoordinateSystem::G54,
                spindle_command(),
                None,
                0,
                0,
            )
            .unwrap_err();

            assert_eq!(error.code, "protocol", "response: {response}");
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
    fn rejects_overflow_when_deriving_offsets_or_machine_positions() {
        for (response, cached_offset) in [
            ("<Idle|MPos:1e308,0,0|WPos:-1e308,0,0|FS:0,0>", None),
            ("<Idle|WPos:1e308,0,0|FS:0,0>", Some([1e308, 0.0, 0.0])),
        ] {
            let error = GrblAdapter::parse_status(
                response,
                cached_offset,
                CoordinateSystem::G54,
                spindle_command(),
                None,
                0,
                0,
            )
            .unwrap_err();

            assert_eq!(error.code, "protocol");
        }
    }

    #[test]
    fn validates_grbl_state_subcodes() {
        for state in [
            "Idle", "Run", "Jog", "Check", "Home", "Sleep", "Hold:0", "Hold:1", "Door:0", "Door:3",
            "Alarm", "Alarm:1",
        ] {
            assert!(GrblAdapter::is_known_machine_state(state), "state: {state}");
        }

        for state in [
            "Idle:1",
            "Run:0",
            "Hold:2",
            "Door:4",
            "Door:invalid",
            "Alarm:0",
            "Alarm:invalid",
        ] {
            assert!(
                !GrblAdapter::is_known_machine_state(state),
                "state: {state}"
            );
        }
    }

    #[test]
    fn rejects_unknown_or_empty_grbl_machine_states() {
        for response in [
            "<|MPos:0,0,0|FS:0,0>",
            "<Unknown|MPos:0,0,0|FS:0,0>",
            "<Idleish|MPos:0,0,0|FS:0,0>",
            "<Runaway|MPos:0,0,0|FS:0,0>",
            "<Hold:invalid|MPos:0,0,0|FS:0,0>",
        ] {
            let error = GrblAdapter::parse_status(
                response,
                None,
                CoordinateSystem::G54,
                spindle_command(),
                None,
                0,
                0,
            )
            .unwrap_err();

            assert_eq!(error.code, "protocol", "response: {response}");
        }
    }

    #[test]
    fn accepts_machine_position_when_grbl_omits_work_offset() {
        let status = GrblAdapter::parse_status(
            "<Idle|MPos:1,2,3|FS:0,0>",
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap();

        assert_eq!(status.position.x, 1.0);
        assert_eq!(status.position.y, 2.0);
        assert_eq!(status.position.z, 3.0);
        assert_eq!(status.work_offset.x, 0.0);
        assert_eq!(status.work_offset.y, 0.0);
        assert_eq!(status.work_offset.z, 0.0);
    }

    #[test]
    fn rejects_work_position_when_no_offset_is_available() {
        let error = GrblAdapter::parse_status(
            "<Idle|WPos:1,2,3|FS:0,0>",
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap_err();

        assert_eq!(error.code, "protocol");
        assert!(error.message.contains("work coordinate offset"));
    }

    #[test]
    fn maps_grbl_door_interlock_to_paused_state() {
        let status = GrblAdapter::parse_status(
            "<Door:1|MPos:1,2,3|WCO:0,0,0|FS:0,0>",
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap();

        assert_eq!(status.interpreter_state, InterpreterState::Paused);
        assert_eq!(status.message.as_deref(), Some("GRBL Door:1"));
    }

    #[test]
    fn rejects_status_without_position_fields() {
        let error = GrblAdapter::parse_status(
            "<Idle|FS:0,0>",
            None,
            CoordinateSystem::G54,
            spindle_command(),
            None,
            0,
            0,
        )
        .unwrap_err();

        assert_eq!(error.code, "protocol");
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
