use crate::{
    Axis, AxisConfiguration, AxisUnits, CncApi, CncResult, ControllerError, CoordinateSystem,
    InterpreterState, JogRequest, MachineAxis, MachineConfiguration, MachineStatus, Position,
    SpindleCommand, SpindleConfiguration, SpindleDirection, SpindleStatus, TaskMode, TaskState,
};
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashMap, path::Path, time::Duration};
use tokio::{net::TcpStream, sync::Mutex, time::timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const STATUS_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_RESPONSE_LINE: usize = 4096;
const MAX_PROGRAM_BYTES: usize = 5 * 1024 * 1024;
const JOG_CHUNK_MM: f64 = 10.0;

type FluidNcSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Copy)]
struct ContinuousJog {
    x: i8,
    y: i8,
    axis: Option<Axis>,
    feed_rate: f64,
}

struct Session {
    socket: Option<FluidNcSocket>,
    rx_buffer: Vec<u8>,
    work_offset: Option<[f64; 6]>,
    reported_axis_count: usize,
    active_coordinate_system: CoordinateSystem,
    spindle_command: SpindleCommand,
    program: Vec<String>,
    program_name: Option<String>,
    /// Следующая строка для отправки; отдельная строка остаётся активной до Idle.
    current_line: u32,
    active_program_line: Option<u32>,
    program_streaming: bool,
    continuous_jog: Option<ContinuousJog>,
}

pub struct FluidNcWebSocketAdapter {
    url: String,
    session: Mutex<Session>,
}

impl FluidNcWebSocketAdapter {
    pub fn connect(url: impl Into<String>) -> CncResult<Self> {
        let url = url.into();
        if !url.starts_with("ws://") && !url.starts_with("wss://") {
            return Err(ControllerError::new(
                "connect",
                "FluidNC WebSocket URL must start with ws:// or wss://",
            ));
        }

        Ok(Self {
            url,
            session: Mutex::new(Session {
                socket: None,
                rx_buffer: Vec::new(),
                work_offset: None,
                reported_axis_count: 3,
                active_coordinate_system: CoordinateSystem::G54,
                spindle_command: SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
                program: Vec::new(),
                program_name: None,
                current_line: 0,
                active_program_line: None,
                program_streaming: false,
                continuous_jog: None,
            }),
        })
    }

    async fn ensure_connected(&self, session: &mut Session) -> CncResult<()> {
        if session.socket.is_some() {
            return Ok(());
        }

        let (socket, _) = timeout(RESPONSE_TIMEOUT, connect_async(&self.url))
            .await
            .map_err(|_| ControllerError::new("timeout", "timed out connecting to FluidNC"))?
            .map_err(|error| ControllerError::new("connect", error.to_string()))?;
        session.socket = Some(socket);
        session.rx_buffer.clear();
        Ok(())
    }

    fn invalidate_connection(session: &mut Session) {
        session.socket = None;
        session.rx_buffer.clear();
        session.program_streaming = false;
        session.continuous_jog = None;
    }

    fn trace_frame(direction: &str, opcode: &str, payload: &[u8]) {
        if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
            eprintln!(
                "[FluidNC WS] {direction} {opcode} len={} payload={:?}",
                payload.len(),
                String::from_utf8_lossy(payload),
            );
        }
    }

    fn trace_binary_response(payload: &[u8]) {
        if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_none() {
            return;
        }

        for line in payload.split(|byte| *byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() || (line.starts_with(b"<") && line.ends_with(b">")) {
                continue;
            }
            Self::trace_frame("recv", "binary", line);
        }
    }

    async fn send_text(session: &mut Session, text: &str) -> CncResult<()> {
        // FluidNC завершает G-code-команду переводом строки. Например, кадр
        // WebUI "$G" длиной 3 байта содержит "$G\n".
        let payload = format!("{}\n", text.trim_end_matches(['\r', '\n']));
        Self::trace_frame("send", "text", payload.as_bytes());
        let result = match session.socket.as_mut() {
            Some(socket) => timeout(RESPONSE_TIMEOUT, socket.send(Message::Text(payload)))
                .await
                .map_err(|_| {
                    ControllerError::new("timeout", "timed out sending command to FluidNC")
                })?
                .map_err(|error| ControllerError::new("io", error.to_string())),
            None => Err(ControllerError::new(
                "connection",
                "FluidNC WebSocket is disconnected",
            )),
        };

        if result.is_err() {
            Self::invalidate_connection(session);
        }
        result
    }

    async fn send_realtime(session: &mut Session, byte: u8) -> CncResult<()> {
        if byte != b'?' {
            Self::trace_frame("send", "binary", &[byte]);
        }
        let result = match session.socket.as_mut() {
            Some(socket) => timeout(RESPONSE_TIMEOUT, socket.send(Message::Binary(vec![byte])))
                .await
                .map_err(|_| {
                    ControllerError::new("timeout", "timed out sending realtime command to FluidNC")
                })?
                .map_err(|error| ControllerError::new("io", error.to_string())),
            None => Err(ControllerError::new(
                "connection",
                "FluidNC WebSocket is disconnected",
            )),
        };

        if result.is_err() {
            Self::invalidate_connection(session);
        }
        result
    }

    fn extract_line(buffer: &mut Vec<u8>) -> CncResult<Option<String>> {
        let Some(end) = buffer.iter().position(|byte| *byte == b'\n') else {
            if buffer.len() > MAX_RESPONSE_LINE {
                return Err(ControllerError::new(
                    "protocol",
                    "FluidNC response line exceeds size limit",
                ));
            }
            return Ok(None);
        };

        if end > MAX_RESPONSE_LINE {
            return Err(ControllerError::new(
                "protocol",
                "FluidNC response line exceeds size limit",
            ));
        }

        let mut line: Vec<u8> = buffer.drain(..=end).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        Ok(Some(String::from_utf8_lossy(&line).trim().to_owned()))
    }

    async fn read_line(session: &mut Session, duration: Duration) -> CncResult<String> {
        loop {
            if let Some(line) = Self::extract_line(&mut session.rx_buffer)? {
                return Ok(line);
            }

            let socket = session.socket.as_mut().ok_or_else(|| {
                ControllerError::new("connection", "FluidNC WebSocket is disconnected")
            })?;
            let message = timeout(duration, socket.next())
                .await
                .map_err(|_| {
                    ControllerError::new("timeout", "timed out waiting for FluidNC response")
                })?
                .ok_or_else(|| ControllerError::new("disconnected", "FluidNC WebSocket closed"))?
                .map_err(|error| ControllerError::new("io", error.to_string()))?;

            match message {
                Message::Text(text) => {
                    Self::trace_frame("recv", "text", text.as_bytes());
                    let text = text.as_str();
                    if !text.starts_with("currentID:") && !text.starts_with("CURRENT_ID:") {
                        session.rx_buffer.extend_from_slice(text.as_bytes());
                    }
                }
                Message::Binary(bytes) => {
                    Self::trace_binary_response(&bytes);
                    session.rx_buffer.extend_from_slice(&bytes);
                }
                Message::Ping(_) | Message::Pong(_) => {}
                Message::Close(_) => {
                    session.socket = None;
                    return Err(ControllerError::new(
                        "disconnected",
                        "FluidNC WebSocket closed",
                    ));
                }
                _ => {}
            }
        }
    }

    async fn send_command(session: &mut Session, command: &str) -> CncResult<()> {
        if command.contains(['\n', '\r']) {
            return Err(ControllerError::new(
                "bad_command",
                "a FluidNC command must contain exactly one line",
            ));
        }
        if command.trim().is_empty() {
            return Ok(());
        }

        Self::send_text(session, command.trim()).await?;
        let result = match timeout(COMMAND_TIMEOUT, async {
            loop {
                let line = Self::read_line(session, COMMAND_TIMEOUT).await?;
                if line == "ok" {
                    if let Some(system) = Self::coordinate_system_select(command) {
                        session.active_coordinate_system = system;
                    }
                    return Ok(());
                }
                if let Some(error) = line.strip_prefix("error:") {
                    return Err(ControllerError::new(
                        "gcode_error",
                        format!("FluidNC rejected command ({error}): {command}"),
                    ));
                }
                if line.starts_with("ALARM:") {
                    return Err(ControllerError::new("alarm", line));
                }
                // FluidNC may interleave status reports, startup banners and [GC:...] messages.
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC command acknowledgement",
            )),
        };

        if result
            .as_ref()
            .is_err_and(|error| error.code != "gcode_error" && error.code != "alarm")
        {
            Self::invalidate_connection(session);
        }
        result
    }

    async fn query_status(session: &mut Session) -> CncResult<String> {
        Self::send_realtime(session, b'?').await?;
        let result = match timeout(STATUS_TIMEOUT, async {
            loop {
                let line = Self::read_line(session, STATUS_TIMEOUT).await?;
                if line.starts_with('<') && line.ends_with('>') {
                    return Ok(line);
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC status",
            )),
        };

        if let Err(error) = &result {
            if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
                eprintln!(
                    "[FluidNC WS] status query failed: {}: {}; buffered={:?}",
                    error.code,
                    error.message,
                    String::from_utf8_lossy(&session.rx_buffer),
                );
            }
            if error.code != "timeout" {
                Self::invalidate_connection(session);
            }
        }
        result
    }

    async fn query_settings(session: &mut Session) -> CncResult<HashMap<u16, f64>> {
        Self::send_text(session, "$$").await?;

        let result = match timeout(COMMAND_TIMEOUT, async {
            let mut settings = HashMap::new();
            loop {
                let line = Self::read_line(session, COMMAND_TIMEOUT).await?;

                if line == "ok" {
                    return Ok(settings);
                }
                if let Some(error) = line.strip_prefix("error:") {
                    return Err(ControllerError::new("settings", error));
                }
                if line.starts_with("ALARM:") {
                    return Err(ControllerError::new("alarm", line));
                }

                let Some((number, value)) = line
                    .strip_prefix('$')
                    .and_then(|setting| setting.split_once('='))
                else {
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
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC settings",
            )),
        };

        if result
            .as_ref()
            .is_err_and(|error| error.code != "settings" && error.code != "alarm")
        {
            Self::invalidate_connection(session);
        }
        result
    }

    fn reported_axis_count(response: &str) -> Option<usize> {
        response
            .split('|')
            .find(|field| field.starts_with("MPos:") || field.starts_with("WPos:"))
            .and_then(|field| field.split_once(':'))
            .map(|(_, values)| values.split(',').count())
            .filter(|count| (3..=6).contains(count))
    }

    fn parse_axes(value: &str) -> Option<[f64; 6]> {
        let mut axes = [0.0_f64; 6];
        let mut count = 0;

        for component in value.split(',') {
            if count == axes.len() {
                return None;
            }
            axes[count] = component.parse().ok()?;
            if !axes[count].is_finite() {
                return None;
            }
            count += 1;
        }

        (count >= 3).then_some(axes)
    }

    fn parse_status(session: &Session, response: &str) -> CncResult<MachineStatus> {
        let body = response
            .strip_prefix('<')
            .and_then(|value| value.strip_suffix('>'))
            .ok_or_else(|| ControllerError::new("protocol", "malformed FluidNC status response"))?;

        let mut fields = body.split('|');
        let state = fields.next().unwrap_or("Unknown");
        let mut machine_position: Option<[f64; 6]> = None;
        let mut work_position: Option<[f64; 6]> = None;
        let mut work_offset: Option<[f64; 6]> = None;
        let mut feed_rate = 0.0;
        let mut spindle_rpm = 0.0;

        for field in fields {
            if let Some(value) = field.strip_prefix("MPos:") {
                machine_position = Self::parse_axes(value);
            } else if let Some(value) = field.strip_prefix("WPos:") {
                work_position = Self::parse_axes(value);
            } else if let Some(value) = field.strip_prefix("WCO:") {
                work_offset = Self::parse_axes(value);
            } else if let Some(value) = field.strip_prefix("FS:") {
                let mut values = value.split(',');
                feed_rate = values
                    .next()
                    .and_then(|item| item.parse().ok())
                    .unwrap_or(0.0);
                spindle_rpm = values
                    .next()
                    .and_then(|item| item.parse().ok())
                    .unwrap_or(0.0);
            } else if let Some(value) = field.strip_prefix("F:") {
                feed_rate = value.parse().unwrap_or(0.0);
            }
        }

        let offset = work_offset
            .or_else(|| match (machine_position, work_position) {
                (Some(machine), Some(work)) => {
                    Some(std::array::from_fn(|axis| machine[axis] - work[axis]))
                }
                _ => None,
            })
            .or(session.work_offset)
            .unwrap_or([0.0; 6]);
        let machine = machine_position
            .or_else(|| {
                work_position.map(|work| std::array::from_fn(|axis| work[axis] + offset[axis]))
            })
            .ok_or_else(|| {
                ControllerError::new("protocol", "FluidNC status contains no valid position")
            })?;

        let lower_state = state.to_ascii_lowercase();
        let alarm = lower_state.starts_with("alarm");
        let running = lower_state == "run";
        let jogging = lower_state == "jog";
        let paused = lower_state.starts_with("hold") || lower_state.starts_with("door");

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
                a: machine[3],
                b: machine[4],
                c: machine[5],
            },
            work_offset: Position {
                x: offset[0],
                y: offset[1],
                z: offset[2],
                a: offset[3],
                b: offset[4],
                c: offset[5],
            },
            active_coordinate_system: session.active_coordinate_system,
            coordinate_systems: Vec::new(),
            feed_rate: if jogging { 0.0 } else { feed_rate },
            spindle: SpindleStatus {
                commanded_rpm: session.spindle_command.rpm,
                actual_rpm: spindle_rpm,
                direction: session.spindle_command.direction,
                enabled: spindle_rpm > 0.0 || session.spindle_command.rpm > 0.0,
            },
            mist_coolant_on: false,
            flood_coolant_on: false,
            jog_rate: if jogging { feed_rate } else { 0.0 },
            motion_mode: if jogging {
                Some("Jog".into())
            } else if running {
                Some("G-code".into())
            } else {
                None
            },
            max_spindle_rpm: 0.0,
            loaded_program: session.program_name.clone(),
            current_line: session.current_line,
            total_lines: session.program.len() as u32,
            message: Some(format!("FluidNC {state}")),
            homed: [false; 6],
            homing: lower_state == "home",
        })
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

    fn coordinate_system_select(command: &str) -> Option<CoordinateSystem> {
        let command = command.to_ascii_uppercase();
        CoordinateSystem::ALL.into_iter().find(|system| {
            command
                .split_whitespace()
                .any(|word| word == system.label())
        })
    }

    async fn send_next_program_line(session: &mut Session) -> CncResult<()> {
        if !session.program_streaming || session.active_program_line.is_some() {
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
            session.active_program_line = None;
            return Err(error);
        }

        // ACK подтверждает приём G-code, а не завершение движения. Не переключаем
        // viewer на следующую траекторию до Idle-статуса после выполнения строки.
        session.active_program_line = Some(index as u32);
        session.current_line = (index + 1) as u32;
        Ok(())
    }

    async fn send_jog_chunk(session: &mut Session, jog: ContinuousJog) -> CncResult<()> {
        let command = if let Some(axis) = jog.axis {
            format!(
                "$J=G21 G91 {}{:.3} F{:.0}",
                Self::axis_letter(axis),
                JOG_CHUNK_MM * f64::from(jog.x),
                jog.feed_rate,
            )
        } else {
            let norm = f64::from(jog.x * jog.x + jog.y * jog.y).sqrt();
            let x = JOG_CHUNK_MM * f64::from(jog.x) / norm;
            let y = JOG_CHUNK_MM * f64::from(jog.y) / norm;
            format!("$J=G21 G91 X{x:.3} Y{y:.3} F{:.0}", jog.feed_rate)
        };
        Self::send_command(session, &command).await
    }
}

#[async_trait::async_trait]
impl CncApi for FluidNcWebSocketAdapter {
    async fn status(&self) -> CncResult<MachineStatus> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let response = Self::query_status(&mut session).await?;
        let mut status = Self::parse_status(&session, &response)?;
        if let Some(count) = Self::reported_axis_count(&response) {
            session.reported_axis_count = count;
        }

        let idle = response
            .strip_prefix('<')
            .and_then(|value| value.split('|').next())
            .is_some_and(|state| state.eq_ignore_ascii_case("Idle"));

        if idle && session.program_streaming {
            // Только Idle означает, что активная строка завершила движение.
            if session.active_program_line.take().is_some()
                && session.current_line as usize >= session.program.len()
            {
                session.program_streaming = false;
            }

            if session.program_streaming {
                Self::send_next_program_line(&mut session).await?;
            }
        } else if idle {
            if let Some(jog) = session.continuous_jog {
                Self::send_jog_chunk(&mut session, jog).await?;
            }
        }

        status.current_line = session
            .active_program_line
            .map_or(session.current_line, |line| line + 1);
        if session.active_program_line.is_some() {
            status.task_mode = TaskMode::Auto;
            if status.interpreter_state == InterpreterState::Idle {
                status.interpreter_state = InterpreterState::Reading;
            }
        }

        if response.contains("|WCO:")
            || (response.contains("|MPos:") && response.contains("|WPos:"))
        {
            session.work_offset = Some([
                status.work_offset.x,
                status.work_offset.y,
                status.work_offset.z,
                status.work_offset.a,
                status.work_offset.b,
                status.work_offset.c,
            ]);
        }
        Ok(status)
    }

    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;

        let status = Self::query_status(&mut session).await?;
        if let Some(count) = Self::reported_axis_count(&status) {
            session.reported_axis_count = count;
        }

        // FluidNC exposes GRBL-compatible travel and spindle limits through $$:
        // $130-$135 are the maximum travels for X/Y/Z/A/B/C, $30/$31 spindle RPM.
        let settings = Self::query_settings(&mut session).await?;
        let axis_definitions = [
            (MachineAxis::X, AxisUnits::Linear),
            (MachineAxis::Y, AxisUnits::Linear),
            (MachineAxis::Z, AxisUnits::Linear),
            (MachineAxis::A, AxisUnits::Angular),
            (MachineAxis::B, AxisUnits::Angular),
            (MachineAxis::C, AxisUnits::Angular),
        ];
        let axes = axis_definitions
            .into_iter()
            .enumerate()
            .take(session.reported_axis_count)
            .map(|(index, (axis, units))| AxisConfiguration {
                axis,
                units,
                max_travel: settings
                    .get(&(130 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
                max_velocity: settings
                    .get(&(110 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
                acceleration: settings
                    .get(&(120 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
                steps_per_unit: settings
                    .get(&(100 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
            })
            .collect();

        let min_rpm = settings.get(&31).copied().filter(|value| *value >= 0.0);
        let max_rpm = settings
            .get(&30)
            .copied()
            .filter(|value| *value > 0.0)
            .filter(|maximum| min_rpm.is_none_or(|minimum| minimum <= *maximum));

        Ok(MachineConfiguration {
            axes,
            spindle: SpindleConfiguration { min_rpm, max_rpm },
            settings,
        })
    }

    async fn power_on(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        Self::send_command(&mut session, "$X").await
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

    async fn home(&self, axis: Option<Axis>) -> CncResult<()> {
        if axis.is_some() {
            return Err(ControllerError::new(
                "unsupported",
                "FluidNC $H homes all configured axes; individual-axis homing is unsupported",
            ));
        }
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        Self::send_command(&mut session, "$H").await
    }

    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        Self::send_command(&mut session, system.label()).await?;
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
        let command = format!("G10 L20 P{} {axes}", Self::coordinate_number(system));
        Self::send_command(&mut session, &command).await?;
        session.work_offset = None;
        Ok(())
    }

    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let command = format!(
            "G10 L20 P{} {}0",
            Self::coordinate_number(session.active_coordinate_system),
            Self::axis_letter(axis),
        );
        Self::send_command(&mut session, &command).await?;
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
            Self::axis_letter(request.axis),
            request.distance * f64::from(request.direction),
        );
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
        Self::send_jog_chunk(&mut session, jog).await
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
        Self::send_jog_chunk(&mut session, jog).await
    }

    async fn cancel_jog(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.continuous_jog = None;
        Self::send_realtime(&mut session, 0x85).await
    }

    async fn stop_motion(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let jogging = session.continuous_jog.take().is_some();
        Self::send_realtime(&mut session, if jogging { 0x85 } else { b'!' }).await
    }

    async fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        Self::send_command(&mut session, command).await
    }

    async fn load_program(&self, path: &Path) -> CncResult<()> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|error| ControllerError::new("program_read", error.to_string()))?;
        if content.len() > MAX_PROGRAM_BYTES {
            return Err(ControllerError::new(
                "bad_program",
                format!("FluidNC program exceeds the {MAX_PROGRAM_BYTES}-byte size limit"),
            ));
        }

        let lines = content
            .lines()
            .map(|line| line.split(';').next().unwrap_or("").trim())
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("program.nc")
            .to_owned();

        let mut session = self.session.lock().await;
        session.program = lines;
        session.program_name = Some(name);
        session.current_line = 0;
        session.active_program_line = None;
        session.program_streaming = false;
        Ok(())
    }

    async fn program_run(&self, start_line: u32) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let status = Self::query_status(&mut session).await?;
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
        session.program_streaming = true;
        Self::send_next_program_line(&mut session).await
    }

    async fn program_pause(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        Self::send_realtime(&mut session, b'!').await
    }

    async fn program_resume(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        Self::send_realtime(&mut session, b'~').await
    }

    async fn program_abort(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.program_streaming = false;
        session.continuous_jog = None;
        session.current_line = 0;
        Self::send_realtime(&mut session, 0x18).await
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
        Self::send_command(&mut session, &gcode).await?;
        session.spindle_command = normalized;
        Ok(())
    }

    async fn clear_error(&self) -> CncResult<()> {
        self.power_on().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_axis_count_from_fluidnc_status_fields() {
        assert_eq!(
            FluidNcWebSocketAdapter::reported_axis_count("<Idle|MPos:1,2,3,4,5,6|FS:0,0>"),
            Some(6)
        );
        assert_eq!(
            FluidNcWebSocketAdapter::reported_axis_count("<Idle|WPos:1,2,3|FS:0,0>"),
            Some(3)
        );
        assert_eq!(
            FluidNcWebSocketAdapter::reported_axis_count("<Idle|FS:0,0>"),
            None
        );
    }
}
