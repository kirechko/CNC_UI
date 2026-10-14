use super::dynamics::{ActiveMove, Motion};
use super::protocol::{api_error, Request, WireError};
use serde_json::Value;

use super::config::EmulatorConfig;
use crate::cnc_api::*;
use std::path::PathBuf;
use std::time::Instant;

#[path = "state/commands.rs"]
mod commands;
#[path = "state/coordinates.rs"]
mod coordinates;
#[path = "state/tick.rs"]
mod tick;

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


    pub(super) fn queue_move(&mut self, target: [f64; 3], feed: f64, rapid: bool) -> Result<(), WireError> {
        self.check_target(target)?;
        let motion = Motion::new(self.position_array(), target, feed, rapid, &self.config)?;
        self.active_move = motion.map(ActiveMove::from_motion);
        Ok(())
    }

    pub(super) fn select_coordinate_system_from_gcode(&mut self, system: CoordinateSystem) {
        self.select_coordinate_system(system);
    }

    pub(super) fn persist_coordinate_offsets_from_gcode(&self) -> Result<(), WireError> {
        self.persist_coordinate_offsets()
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

            "power_on" => commands::power_on(self),

            "power_off" => commands::power_off(self, false),
            "estop" => commands::power_off(self, true),

            "select_coordinate_system" => commands::select_coordinate_system(self, args),
            "set_work_zero" => commands::set_work_zero(self, args),

            "home" => commands::home(self, args),

            "jog_increment" => commands::jog_increment(self, args),

            "jog_continuous" => commands::jog_continuous(self, args),

            "mdi" => {
                self.ensure_motion_allowed()?;
                self.status.task_mode = TaskMode::Mdi;
                self.execute_gcode(args.get("command").and_then(Value::as_str).unwrap_or(""))?;
                Ok(Value::Null)
            }

            "load_program" => commands::load_program(self, args),

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
        tick::tick(self, dt);
    }
}

fn axis_index(axis: Axis) -> usize {
    match axis { Axis::X => 0, Axis::Y => 1, Axis::Z => 2 }
}


#[cfg(test)]
mod tests {
    use super::Emulator;
    use super::super::config::{
        AxesConfig, AxisLimits, EmulatorConfig, SpindleConfig,
    };
    use crate::cnc_api::{CoordinateSystem, CoordinateSystemOffset, Position};
    use std::path::PathBuf;

    fn test_emulator() -> Emulator {
        let config = EmulatorConfig {
            axes: AxesConfig {
                x: AxisLimits {
                    min_mm: 0.0, max_mm: 300.0,
                    max_velocity_mm_min: 6000.0, acceleration_mm_s2: 100.0,
                },
                y: AxisLimits {
                    min_mm: 0.0, max_mm: 400.0,
                    max_velocity_mm_min: 6000.0, acceleration_mm_s2: 100.0,
                },
                z: AxisLimits {
                    min_mm: 0.0, max_mm: 100.0,
                    max_velocity_mm_min: 3000.0, acceleration_mm_s2: 50.0,
                },
            },
            spindle: SpindleConfig {
                max_rpm: 24_000.0,
                acceleration_rpm_per_sec: 1000.0,
            },
        };
        let offsets = vec![CoordinateSystemOffset {
            system: CoordinateSystem::G54,
            offset: Position { x: 0.0, y: 0.0, z: 0.0 },
        }];
        Emulator::new(config, PathBuf::from("unused-test-offsets.yaml"), offsets)
    }

    #[test]
    fn target_inside_limits_is_accepted() {
        assert!(test_emulator().check_target([150.0, 200.0, 50.0]).is_ok());
    }

    #[test]
    fn exact_axis_boundaries_are_accepted() {
        let emulator = test_emulator();
        assert!(emulator.check_target([0.0, 0.0, 0.0]).is_ok());
        assert!(emulator.check_target([300.0, 400.0, 100.0]).is_ok());
    }

    #[test]
    fn targets_below_axis_minima_are_rejected() {
        let emulator = test_emulator();
        assert!(emulator.check_target([-0.001, 100.0, 50.0]).is_err());
        assert!(emulator.check_target([100.0, -0.001, 50.0]).is_err());
        assert!(emulator.check_target([100.0, 100.0, -0.001]).is_err());
    }

    #[test]
    fn targets_above_axis_maxima_are_rejected() {
        let emulator = test_emulator();
        assert!(emulator.check_target([300.001, 100.0, 50.0]).is_err());
        assert!(emulator.check_target([100.0, 400.001, 50.0]).is_err());
        assert!(emulator.check_target([100.0, 100.0, 100.001]).is_err());
    }

    #[test]
    fn small_floating_point_tolerance_is_allowed() {
        let emulator = test_emulator();
        assert!(emulator.check_target([-5e-9, 100.0, 50.0]).is_ok());
        assert!(emulator.check_target([300.0 + 5e-9, 100.0, 50.0]).is_ok());
    }
}

#[cfg(test)]
mod load_program_comment_tests {
    use super::Emulator;
    use super::super::config::{
        AxesConfig, AxisLimits, EmulatorConfig, SpindleConfig,
    };
    use super::super::protocol::Request;
    use crate::cnc_api::{CoordinateSystem, CoordinateSystemOffset, Position};
    use std::path::PathBuf;

    fn test_emulator() -> Emulator {
        let config = EmulatorConfig {
            axes: AxesConfig {
                x: AxisLimits {
                    min_mm: 0.0, max_mm: 300.0,
                    max_velocity_mm_min: 6000.0, acceleration_mm_s2: 100.0,
                },
                y: AxisLimits {
                    min_mm: 0.0, max_mm: 400.0,
                    max_velocity_mm_min: 6000.0, acceleration_mm_s2: 100.0,
                },
                z: AxisLimits {
                    min_mm: 0.0, max_mm: 100.0,
                    max_velocity_mm_min: 3000.0, acceleration_mm_s2: 50.0,
                },
            },
            spindle: SpindleConfig {
                max_rpm: 24_000.0,
                acceleration_rpm_per_sec: 1000.0,
            },
        };

        let offsets = vec![CoordinateSystemOffset {
            system: CoordinateSystem::G54,
            offset: Position { x: 0.0, y: 0.0, z: 0.0 },
        }];

        Emulator::new(
            config,
            PathBuf::from("unused-test-offsets.yaml"),
            offsets,
        )
    }

    #[test]
    fn load_program_strips_semicolon_comments_but_keeps_parenthesis_comments() {
        let mut emulator = test_emulator();

        let request = Request {
            method: "load_program".into(),
            args: serde_json::json!({
                "name": "comments.ngc",
                "content": "G1 X10 ; M8 X20\nG1 X30 (G99 Y42)\n"
            }),
        };

        emulator.handle(request).unwrap();

        assert_eq!(
            emulator.program,
            vec![
                "G1 X10".to_string(),
                "G1 X30 (G99 Y42)".to_string(),
            ]
        );
    }
}

