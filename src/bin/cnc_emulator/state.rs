//! Stores emulator state and delegates commands, coordinates, and ticking to submodules.
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
#[cfg(test)]
#[path = "state/tests.rs"]
mod tests;
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
    pub(super) continuous_jog_xy: Option<(i8, i8, f64)>,
    pub(super) jog_move_active: bool,
    /// Направление и ускорение для плавной остановки непрерывного Jog.
    pub(super) jog_stop: Option<([f64; 5], f64)>,
    pub(super) active_move_is_rapid: bool,
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
    pub(super) fn new(
        config: EmulatorConfig,
        coordinate_offsets_path: PathBuf,
        offsets: Vec<CoordinateSystemOffset>,
    ) -> Self {
        Self {
            status: MachineStatus {
                connected: true,
                task_mode: TaskMode::Manual,
                task_state: TaskState::On,
                interpreter_state: InterpreterState::Idle,
                position: Position {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    a: 0.0,
                    b: 0.0,
                },
                work_offset: offsets
                    .iter()
                    .find(|entry| entry.system == CoordinateSystem::G54)
                    .map(|entry| entry.offset)
                    .unwrap_or(Position {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0,
                        a: 0.0,
                        b: 0.0,
                    }),
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
                homed: [false; 5],
                homing: false,
                max_spindle_rpm: config.spindle.max_rpm,
                mist_coolant_on: false,
                flood_coolant_on: false,
                jog_rate: 0.0,
                motion_mode: None,
            },
            config,
            coordinate_offsets_path,
            program: Vec::new(),
            dwell_remaining: 0.0,
            absolute_mode: true,
            active_move: None,
            continuous_jog: None,
            continuous_jog_xy: None,
            jog_move_active: false,
            jog_stop: None,
            active_move_is_rapid: false,
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

    pub(super) fn position_array(&self) -> [f64; 5] {
        [
            self.status.position.x,
            self.status.position.y,
            self.status.position.z,
            self.status.position.a,
            self.status.position.b,
        ]
    }

    fn set_position(&mut self, p: [f64; 5]) {
        self.status.position = Position {
            x: p[0],
            y: p[1],
            z: p[2],
            a: p[3],
            b: p[4],
        };
    }

    pub(super) fn check_target(&self, target: [f64; 5]) -> Result<(), WireError> {
        for axis in self.config.configured_axes() {
            let index = axis_index(axis);
            let (minimum, maximum, _, _) = self.config.axis_limits(axis);
            if target[index] < minimum - 1e-8 || target[index] > maximum + 1e-8 {
                let units = if matches!(axis, Axis::A | Axis::B) {
                    "°"
                } else {
                    "mm"
                };
                return Err(api_error(
                    "soft_limit",
                    format!(
                        "{axis:?} target {:.3} outside [{minimum:.3}, {maximum:.3}] {units}",
                        target[index]
                    ),
                ));
            }
        }
        Ok(())
    }

    pub(super) fn queue_move(
        &mut self,
        target: [f64; 5],
        feed: f64,
        rapid: bool,
    ) -> Result<(), WireError> {
        self.check_target(target)?;
        let motion = Motion::new(self.position_array(), target, feed, rapid, &self.config)?;
        self.active_move = motion.map(ActiveMove::from_motion);
        self.active_move_is_rapid = rapid && self.active_move.is_some();
        Ok(())
    }

    pub(super) fn select_coordinate_system_from_gcode(&mut self, system: CoordinateSystem) {
        self.select_coordinate_system(system);
    }

    pub(super) fn persist_coordinate_offsets_from_gcode(&self) -> Result<(), WireError> {
        self.persist_coordinate_offsets()
    }

    pub(super) fn handle(&mut self, request: Request) -> Result<Value, WireError> {
        commands::handle(self, request)
    }

    pub(super) fn tick(&mut self, dt: f64) {
        tick::tick(self, dt);
    }
}

pub(super) fn axis_index(axis: Axis) -> usize {
    match axis {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
        Axis::A => 3,
        Axis::B => 4,
    }
}
