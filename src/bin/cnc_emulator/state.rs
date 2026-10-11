use super::dynamics::{ActiveMove, Motion};
use super::protocol::{api_error, Request, WireError};
use serde_json::Value;
use std::fs;

use super::config::EmulatorConfig;
use crate::cnc_api::*;
use std::path::PathBuf;
use std::time::Instant;

pub(super) struct Emulator {
    pub(super) status: MachineStatus,
    pub(super) config: EmulatorConfig,
    pub(super) coordinate_offsets_path: PathBuf,
    pub(super) program: Vec<String>,
    pub(super) dwell_remaining: f64,
    pub(super) absolute_mode: bool,
    pub(super) active_move: Option<ActiveMove>,
    pub(super) continuous_jog: Option<(Axis, i8, f64)>,
    pub(super) homing_queue: Vec<Axis>,
    pub(super) homing_axis: Option<Axis>,
    pub(super) active_move_is_homing: bool,
    pub(super) pending_spindle: Option<(SpindleDirection, f64)>,
    pub(super) last_rapid_z: Option<f64>,
    pub(super) stop_retract_active: bool,
    pub(super) jog_velocity: f64,
    pub(super) last_tick: Instant,
}

impl Emulator {
    pub(super) fn new(config: EmulatorConfig, coordinate_offsets_path: PathBuf, offsets: Vec<CoordinateSystemOffset>) -> Self {
        Self {
            status: MachineStatus {
                connected: true,
                task_mode: TaskMode::Manual,
                task_state: TaskState::On,
                interpreter_state: InterpreterState::Idle,
                position: Position { x: 0.0, y: 0.0, z: 0.0 },
                work_offset: offsets
                    .iter()
                    .find(|entry| entry.system == CoordinateSystem::G54)
                    .map(|entry| entry.offset)
                    .unwrap_or(Position { x: 0.0, y: 0.0, z: 0.0 }),
                active_coordinate_system: CoordinateSystem::G54,
                coordinate_systems: offsets,
                feed_rate: 100.0,
                spindle: SpindleStatus {
                    commanded_rpm: 0.0,
                    actual_rpm: 0.0,
                    direction: SpindleDirection::Stopped,
                    enabled: false,
                },
                loaded_program: None,
                current_line: 0,
                total_lines: 0,
                message: Some("Virtual controller ready".into()),
                homed: [false; 3],
                homing: false,
                max_spindle_rpm: config.spindle.max_rpm,
                mist_coolant_on: false,
                flood_coolant_on: false,
            },
            config,
            coordinate_offsets_path,
            program: Vec::new(),
            dwell_remaining: 0.0,
            absolute_mode: true,
            active_move: None,
            continuous_jog: None,
            homing_queue: Vec::new(),
            homing_axis: None,
            active_move_is_homing: false,
            pending_spindle: None,
            last_rapid_z: None,
            stop_retract_active: false,
            jog_velocity: 0.0,
            last_tick: Instant::now(),
        }
    }

    pub(super) fn select_coordinate_system(&mut self, system: CoordinateSystem) {
        let offset = self.status.coordinate_systems
            .iter()
            .find(|entry| entry.system == system)
            .map(|entry| entry.offset)
            .unwrap_or(Position { x: 0.0, y: 0.0, z: 0.0 });

        self.status.active_coordinate_system = system;
        self.status.work_offset = offset;
        self.status.message = Some(format!("Active coordinate system: {}", system.label()));
    }

    fn set_selected_work_zero(&mut self, system: CoordinateSystem) -> Result<(), WireError> {
        if self.active_move.is_some()
            || self.continuous_jog.is_some()
            || self.status.homing
            || self.status.interpreter_state == InterpreterState::Reading
        {
            return Err(api_error("motion_busy", "cannot set work zero while motion or program execution is active"));
        }

        let offset = self.status.position;
        if let Some(entry) = self.status.coordinate_systems
            .iter_mut()
            .find(|entry| entry.system == system)
        {
            entry.offset = offset;
        } else {
            self.status.coordinate_systems.push(CoordinateSystemOffset { system, offset });
        }

        self.select_coordinate_system(system);
        self.persist_coordinate_offsets()?;
        self.status.message = Some(format!(
            "{} zero set at machine X{:.3} Y{:.3} Z{:.3}",
            system.label(), offset.x, offset.y, offset.z
        ));
        Ok(())
    }

    pub(super) fn persist_coordinate_offsets(&self) -> Result<(), WireError> {
        let yaml = serde_yaml_ng::to_string(&self.status.coordinate_systems)
            .map_err(|e| api_error("coordinates_save", e.to_string()))?;
        if let Some(parent) = self.coordinate_offsets_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| api_error("coordinates_save", e.to_string()))?;
        }
        fs::write(&self.coordinate_offsets_path, yaml)
            .map_err(|e| api_error("coordinates_save", e.to_string()))
    }

    pub(super) fn position_array(&self) -> [f64; 3] {
        [self.status.position.x, self.status.position.y, self.status.position.z]
    }

    fn set_position(&mut self, p: [f64; 3]) {
        self.status.position = Position { x: p[0], y: p[1], z: p[2] };
    }

    pub(super) fn check_target(&self, target: [f64; 3]) -> Result<(), WireError> {
        for (index, axis) in [Axis::X, Axis::Y, Axis::Z].into_iter().enumerate() {
            let limits = self.config.axis(axis);
            if target[index] < limits.min_mm - 1e-8 || target[index] > limits.max_mm + 1e-8 {
                return Err(api_error(
                    "soft_limit",
                    format!("{:?} target {:.3} outside [{:.3}, {:.3}] mm",
                        axis, target[index], limits.min_mm, limits.max_mm),
                ));
            }
        }
        Ok(())
    }

    fn ensure_motion_allowed(&self) -> Result<(), WireError> {
        if self.status.task_state != TaskState::On {
            return Err(api_error("machine_off", "virtual machine is not powered on"));
        }
        Ok(())
    }

    pub(super) fn queue_move(&mut self, target: [f64; 3], feed: f64, rapid: bool) -> Result<(), WireError> {
        self.check_target(target)?;
        let motion = Motion::new(self.position_array(), target, feed, rapid, &self.config)?;
        self.active_move = motion.map(ActiveMove::from_motion);
        Ok(())
    }





    pub(super) fn handle(&mut self, request: Request) -> Result<Value, WireError> {
        if request.method != "status" {
            let mut logged_args = request.args.clone();

            // Не засоряем терминал содержимым загружаемой программы.
            if request.method == "load_program" {
                if let Some(object) = logged_args.as_object_mut() {
                    if let Some(content) = object.remove("content") {
                        let byte_count = content.as_str().map(str::len).unwrap_or(0);
                        object.insert(
                            "content_bytes".into(),
                            Value::from(byte_count as u64),
                        );
                    }
                }
            }

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            eprintln!(
                "[API {timestamp}] {} {}",
                request.method,
                logged_args
            );
        }

        let args = request.args;
        match request.method.as_str() {
            "status" => Ok(serde_json::to_value(&self.status)
                .map_err(|e| api_error("serialization", e.to_string()))?),

            "power_on" => {
                self.status.task_state = TaskState::On;
                self.status.message = Some("Machine powered on".into());
                Ok(Value::Null)
            }

            "power_off" | "estop" => {
                self.dwell_remaining = 0.0;
                self.status.task_state = if request.method == "estop" { TaskState::Estop } else { TaskState::Off };
                self.status.interpreter_state = InterpreterState::Idle;
                self.status.spindle.enabled = false;
                self.status.mist_coolant_on = false;
                self.status.flood_coolant_on = false;
                self.status.spindle.commanded_rpm = 0.0;
                self.status.spindle.actual_rpm = 0.0;
                self.status.spindle.direction = SpindleDirection::Stopped;
                self.pending_spindle = None;
                self.pending_spindle = None;
                self.active_move = None;
                self.continuous_jog = None;
                self.jog_velocity = 0.0;
                self.homing_queue.clear();
                self.homing_axis = None;
                self.active_move_is_homing = false;
                self.status.homing = false;
                if request.method == "estop" {
                    self.status.homed = [false; 3];
                }
                self.status.message = Some(if request.method == "estop" { "Emergency stop" } else { "Machine powered off" }.into());
                Ok(Value::Null)
            }

            "select_coordinate_system" => {
                let system: CoordinateSystem = serde_json::from_value(
                    args.get("system").cloned().unwrap_or(Value::Null)
                ).map_err(|e| api_error("bad_request", e.to_string()))?;
                self.select_coordinate_system(system);
                Ok(Value::Null)
            }

            "set_work_zero" => {
                let system: CoordinateSystem = serde_json::from_value(
                    args.get("system").cloned().unwrap_or(Value::Null)
                ).map_err(|e| api_error("bad_request", e.to_string()))?;
                self.set_selected_work_zero(system)?;
                Ok(Value::Null)
            }

            "home" => {
                self.ensure_motion_allowed()?;
                if self.active_move.is_some()
                    || self.continuous_jog.is_some()
                    || self.status.interpreter_state == InterpreterState::Reading
                    || self.status.homing
                {
                    return Err(api_error("motion_busy", "cannot home while motion or program is active"));
                }

                let requested: Option<Axis> = serde_json::from_value(
                    args.get("axis").cloned().unwrap_or(Value::Null)
                ).map_err(|e| api_error("bad_request", e.to_string()))?;

                self.homing_queue = match requested {
                    Some(axis) => vec![axis],
                    None => vec![Axis::X, Axis::Y, Axis::Z],
                };
                for axis in &self.homing_queue {
                    self.status.homed[axis_index(*axis)] = false;
                }

                self.status.homing = true;
                self.status.task_mode = TaskMode::Manual;
                self.status.message = Some("Homing started".into());
                Ok(Value::Null)
            }

            "jog_increment" => {
                self.ensure_motion_allowed()?;
                if self.active_move.is_some() {
                    return Err(api_error("motion_busy", "another movement is already in progress"));
                }

                let req: JogRequest = serde_json::from_value(
                    args.get("request").cloned().unwrap_or(Value::Null)
                ).map_err(|e| api_error("bad_request", e.to_string()))?;

                if ![-1, 1].contains(&req.direction) || !req.distance.is_finite() || req.distance < 0.0 {
                    return Err(api_error("bad_jog", "invalid jog direction or distance"));
                }

                let index = axis_index(req.axis);
                let feed = req.feed_rate.unwrap_or(self.status.feed_rate);
                let max_feed = self.config.axis(req.axis).max_velocity_mm_min;
                if !feed.is_finite() || feed <= 0.0 {
                    return Err(api_error("bad_feed", "feed rate must be positive"));
                }

                let mut target = self.position_array();
                target[index] += req.distance * f64::from(req.direction);
                self.check_target(target)?;

                self.status.feed_rate = feed.min(max_feed);
                self.status.task_mode = TaskMode::Manual;
                self.status.message = Some(format!(
                    "Jog {:?} {:+.3} at {:.0} mm/min",
                    req.axis, req.distance * f64::from(req.direction), self.status.feed_rate
                ));

                let motion = Motion::new(
                    self.position_array(),
                    target,
                    self.status.feed_rate,
                    false,
                    &self.config,
                )?;
                self.active_move = motion.map(ActiveMove::from_motion);
                Ok(Value::Null)
            }

            "jog_continuous" => {
                self.ensure_motion_allowed()?;
                let axis: Axis = serde_json::from_value(
                    args.get("axis").cloned().unwrap_or(Value::Null)
                ).map_err(|e| api_error("bad_request", e.to_string()))?;
                let direction = args.get("direction").and_then(Value::as_i64).unwrap_or(0) as i8;
                let feed = args.get("feed_rate").and_then(Value::as_f64).unwrap_or(0.0);

                if direction == 0 {
                    self.continuous_jog = None;
                    self.jog_velocity = 0.0;
                } else if ![-1, 1].contains(&direction) || !feed.is_finite() || feed <= 0.0 {
                    return Err(api_error("bad_jog", "invalid continuous jog parameters"));
                } else {
                    self.continuous_jog = Some((axis, direction, feed.min(self.config.axis(axis).max_velocity_mm_min)));
                    self.status.feed_rate = feed.min(self.config.axis(axis).max_velocity_mm_min);
                }
                Ok(Value::Null)
            }

            "mdi" => {
                self.ensure_motion_allowed()?;
                self.status.task_mode = TaskMode::Mdi;
                self.execute_gcode(args.get("command").and_then(Value::as_str).unwrap_or(""))?;
                Ok(Value::Null)
            }

            "load_program" => {
                let name = args.get("name").and_then(Value::as_str).unwrap_or("program.ngc");
                let content = args.get("content").and_then(Value::as_str).unwrap_or("");
                self.program = content.lines()
                    .map(|line| line.split(';').next().unwrap_or("").trim().to_string())
                    .filter(|line| !line.is_empty())
                    .collect();
                self.status.loaded_program = Some(name.to_string());
                self.status.total_lines = self.program.len() as u32;
                self.status.current_line = 0;
                self.status.task_mode = TaskMode::Auto;
                self.status.interpreter_state = InterpreterState::Idle;
                self.status.message = Some(format!("Loaded {} blocks", self.program.len()));
                Ok(Value::Null)
            }

            "program_run" => {
                self.ensure_motion_allowed()?;
                if self.stop_retract_active || self.active_move.is_some() {
                    return Err(api_error("motion_busy", "cannot start while stop retract is active"));
                }
                let start = args.get("start_line").and_then(Value::as_u64).unwrap_or(0) as usize;
                if self.program.is_empty() || start > self.program.len() {
                    return Err(api_error("bad_program", "no program loaded or invalid start line"));
                }
                self.status.current_line = start as u32;
                self.status.task_mode = TaskMode::Auto;
                self.status.interpreter_state = InterpreterState::Reading;
                self.status.message = Some("Program running".into());
                Ok(Value::Null)
            }

            "program_pause" => {
                if self.status.interpreter_state == InterpreterState::Reading
                    && !self.stop_retract_active
                {
                    self.status.interpreter_state = InterpreterState::Paused;
                    self.status.message = Some("Program paused".into());
                }
                Ok(Value::Null)
            }

            "program_resume" => {
                if self.status.interpreter_state == InterpreterState::Paused {
                    self.status.interpreter_state = InterpreterState::Reading;
                }
                Ok(Value::Null)
            }

            "program_abort" => {
                self.dwell_remaining = 0.0;
                self.status.interpreter_state = InterpreterState::Idle;
                self.status.current_line = 0;
                self.active_move = None;
                self.continuous_jog = None;
                self.jog_velocity = 0.0;

                // STOP принудительно гасит шпиндель, не выполняя ramp-down.
                self.pending_spindle = None;
                self.status.spindle.commanded_rpm = 0.0;
                self.status.spindle.actual_rpm = 0.0;
                self.status.spindle.direction = SpindleDirection::Stopped;
                self.status.spindle.enabled = false;
                self.status.mist_coolant_on = false;
                self.status.flood_coolant_on = false;

                self.stop_retract_active = false;
                if let Some(safe_machine_z) = self.last_rapid_z {
                    let mut target = self.position_array();
                    target[2] = safe_machine_z;
                    if self.check_target(target).is_ok() && (target[2] - self.status.position.z).abs() > 1e-6 {
                        let rapid_feed = self.config.axes.z.max_velocity_mm_min;
                        match Motion::new(self.position_array(), target, rapid_feed, true, &self.config) {
                            Ok(Some(motion)) => {
                                self.active_move = Some(ActiveMove::from_motion(motion));
                                self.stop_retract_active = true;
                                self.status.message = Some(format!(
                                    "Program stopped; retracting Z to {:.3} mm",
                                    safe_machine_z - self.status.work_offset.z
                                ));
                                return Ok(Value::Null);
                            }
                            Ok(None) => {}
                            Err(error) => {
                                self.status.message = Some(format!(
                                    "Program stopped; retract failed: {}",
                                    error.message
                                ));
                                return Ok(Value::Null);
                            }
                        }
                    }
                }

                self.status.message = Some("Program stopped; no retract required".into());
                Ok(Value::Null)
            }

            "set_spindle" => {
                if self.status.task_state != TaskState::On {
                    return Err(api_error(
                        "machine_not_ready",
                        "spindle command rejected while machine is in Alarm or powered off",
                    ));
                }

                let command: SpindleCommand = serde_json::from_value(
                    args.get("command").cloned().unwrap_or(Value::Null)
                ).map_err(|e| api_error("bad_request", e.to_string()))?;
                if !command.rpm.is_finite() || command.rpm < 0.0 {
                    return Err(api_error("bad_spindle", "RPM must be finite and non-negative"));
                }
                if command.rpm > self.config.spindle.max_rpm {
                    return Err(api_error(
                        "spindle_limit",
                        format!("requested {:.0} RPM exceeds configured maximum {:.0} RPM",
                            command.rpm, self.config.spindle.max_rpm),
                    ));
                }
                self.set_spindle(command.direction, Some(command.rpm));
                Ok(Value::Null)
            }

            "clear_error" => {
                if self.status.task_state == TaskState::Estop {
                    self.status.task_state = TaskState::EstopReset;
                    self.status.message = Some("Alarm reset".into());
                } else {
                    self.status.message = None;
                }
                Ok(Value::Null)
            }

            _ => Err(api_error("unknown_method", format!("unsupported method: {}", request.method))),
        }
    }

    pub(super) fn tick(&mut self, dt: f64) {
        if self.dwell_remaining > 0.0
            && self.status.interpreter_state != InterpreterState::Paused
        {
            self.dwell_remaining = (self.dwell_remaining - dt).max(0.0);
            if self.dwell_remaining > 0.0 {
                return;
            }
            self.status.message = Some("Dwell complete".into());
            return;
        }
        let target_rpm = self.status.spindle.commanded_rpm;
        let actual_rpm = self.status.spindle.actual_rpm;
        let max_delta = self.config.spindle.acceleration_rpm_per_sec * dt;
        let next_rpm = if actual_rpm < target_rpm {
            (actual_rpm + max_delta).min(target_rpm)
        } else {
            (actual_rpm - max_delta).max(target_rpm)
        };
        self.status.spindle.actual_rpm = next_rpm;
        if target_rpm <= 0.0 && next_rpm <= 0.01 {
            self.status.spindle.actual_rpm = 0.0;

            if let Some((direction, rpm)) = self.pending_spindle.take() {
                self.status.spindle.direction = direction;
                self.status.spindle.commanded_rpm = rpm;
                self.status.spindle.enabled = rpm > 0.0;
            } else {
                self.status.spindle.direction = SpindleDirection::Stopped;
                self.status.spindle.enabled = false;
            }
        } else {
            self.status.spindle.enabled = next_rpm > 0.01 || target_rpm > 0.0;
        }

        let movement_may_advance =
            self.stop_retract_active
                || self.status.interpreter_state != InterpreterState::Paused;

        if movement_may_advance {
            if let Some(mut movement) = self.active_move.take() {
                let (position, done) = movement.tick(dt);
                self.set_position(position);

                if done {
                    if self.stop_retract_active {
                        self.stop_retract_active = false;
                        self.status.message = Some("Stop retract complete".into());
                    }
                    if self.active_move_is_homing {
                    if let Some(axis) = self.homing_axis.take() {
                        self.status.homed[axis_index(axis)] = true;
                    }
                    self.active_move_is_homing = false;
                }
                } else {
                    self.active_move = Some(movement);
                }
            }
        }

        if self.active_move.is_none() && self.status.homing {
            if let Some(axis) = self.homing_queue.first().copied() {
                self.homing_queue.remove(0);
                let mut target = self.position_array();
                let index = axis_index(axis);
                target[index] = self.config.axis(axis).min_mm;
                self.homing_axis = Some(axis);

                match Motion::new(
                    self.position_array(),
                    target,
                    self.config.axis(axis).max_velocity_mm_min,
                    true,
                    &self.config,
                ) {
                    Ok(Some(motion)) => {
                        self.active_move = Some(ActiveMove::from_motion(motion));
                        self.active_move_is_homing = true;
                    }
                    Ok(None) => {
                        self.status.homed[index] = true;
                        self.homing_axis = None;
                    }
                    Err(error) => {
                        self.status.homing = false;
                        self.status.task_state = TaskState::Estop;
                        self.status.message = Some(format!("Homing error: {}", error.message));
                        self.homing_queue.clear();
                        self.homing_axis = None;
                        self.active_move_is_homing = false;
                        return;
                    }
                }
            } else {
                self.status.homing = false;
                self.status.message = Some("Homing complete".into());
            }
        }

        if let Some((axis, direction, feed)) = self.continuous_jog {
            let limits = self.config.axis(axis);
            let max_v = (feed / 60.0).min(limits.max_velocity_mm_min / 60.0);
            self.jog_velocity = (self.jog_velocity + limits.acceleration_mm_s2 * dt).min(max_v);
            let delta = f64::from(direction) * self.jog_velocity * dt;
            let mut p = self.position_array();
            let index = axis_index(axis);
            p[index] = (p[index] + delta).clamp(limits.min_mm, limits.max_mm);
            self.set_position(p);
        } else {
            self.jog_velocity = 0.0;
        }

        if self.status.interpreter_state != InterpreterState::Reading || self.active_move.is_some() || self.status.homing {
            return;
        }

        let index = self.status.current_line as usize;
        if index >= self.program.len() {
            self.status.interpreter_state = InterpreterState::Idle;
            self.status.message = Some("Program finished".into());
            return;
        }

        let line = self.program[index].clone();
        match self.execute_gcode(&line) {
            Ok(()) => {
                self.status.current_line += 1;
                if self.status.current_line as usize >= self.program.len() && self.active_move.is_none() {
                    self.status.interpreter_state = InterpreterState::Idle;
                    self.status.message = Some("Program finished".into());
                }
            }
            Err(error) => {
                self.status.interpreter_state = InterpreterState::Idle;
                self.status.message = Some(format!("{}: {}", error.code, error.message));
            }
        }
    }
}

fn axis_index(axis: Axis) -> usize {
    match axis { Axis::X => 0, Axis::Y => 1, Axis::Z => 2 }
}
