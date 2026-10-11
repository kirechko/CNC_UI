#[allow(dead_code)]
#[path = "../cnc_api.rs"]
mod cnc_api;

use cnc_api::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::f64::consts::PI;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Deserialize)]
struct Request {
    method: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Serialize)]
struct WireError {
    code: String,
    message: String,
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    result: Value,
    error: Option<WireError>,
}

#[derive(Debug, Clone, Deserialize)]
struct AxisLimits {
    min_mm: f64,
    max_mm: f64,
    max_velocity_mm_min: f64,
    acceleration_mm_s2: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct AxesConfig {
    x: AxisLimits,
    y: AxisLimits,
    z: AxisLimits,
}

#[derive(Debug, Clone, Deserialize)]
struct SpindleConfig {
    max_rpm: f64,
    acceleration_rpm_per_sec: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct EmulatorConfig {
    axes: AxesConfig,
    spindle: SpindleConfig,
}

impl EmulatorConfig {
    fn axis(&self, axis: Axis) -> &AxisLimits {
        match axis {
            Axis::X => &self.axes.x,
            Axis::Y => &self.axes.y,
            Axis::Z => &self.axes.z,
        }
    }

    fn validate(&self) -> Result<(), String> {
        for (name, a) in [("X", &self.axes.x), ("Y", &self.axes.y), ("Z", &self.axes.z)] {
            if !a.min_mm.is_finite()
                || !a.max_mm.is_finite()
                || a.min_mm >= a.max_mm
                || !a.max_velocity_mm_min.is_finite()
                || a.max_velocity_mm_min <= 0.0
                || !a.acceleration_mm_s2.is_finite()
                || a.acceleration_mm_s2 <= 0.0
            {
                return Err(format!("invalid configuration for axis {name}"));
            }
        }
        if !self.spindle.max_rpm.is_finite()
            || self.spindle.max_rpm <= 0.0
            || !self.spindle.acceleration_rpm_per_sec.is_finite()
            || self.spindle.acceleration_rpm_per_sec <= 0.0
        {
            return Err("invalid spindle configuration".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Motion {
    start: [f64; 3],
    target: [f64; 3],
    distance: f64,
    max_velocity: f64,    // mm/s along path
    acceleration: f64,   // mm/s^2 along path
}

impl Motion {
    fn new(
        start: [f64; 3],
        target: [f64; 3],
        feed_mm_min: f64,
        rapid: bool,
        config: &EmulatorConfig,
    ) -> Result<Option<Self>, WireError> {
        let delta = [
            target[0] - start[0],
            target[1] - start[1],
            target[2] - start[2],
        ];
        let distance = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        if distance < 1e-9 {
            return Ok(None);
        }

        let mut max_velocity = if rapid {
            f64::INFINITY
        } else {
            (feed_mm_min / 60.0).max(0.001)
        };
        let mut acceleration = f64::INFINITY;

        for (index, axis) in [Axis::X, Axis::Y, Axis::Z].into_iter().enumerate() {
            let component = (delta[index] / distance).abs();
            if component > 1e-9 {
                let limits = config.axis(axis);
                max_velocity = max_velocity.min(limits.max_velocity_mm_min / 60.0 / component);
                acceleration = acceleration.min(limits.acceleration_mm_s2 / component);
            }
        }

        if !max_velocity.is_finite() || !acceleration.is_finite() {
            return Err(api_error("motion_config", "could not calculate motion limits"));
        }

        Ok(Some(Self {
            start,
            target,
            distance,
            max_velocity,
            acceleration,
        }))
    }


}

#[derive(Clone, Copy)]
struct ArcMotion {
    center_x: f64,
    center_y: f64,
    radius: f64,
    start_angle: f64,
    sweep: f64,
    start_z: f64,
    end_z: f64,
}

struct ActiveMove {
    start: [f64; 3],
    target: [f64; 3],
    total_distance: f64,
    travelled: f64,
    velocity: f64,
    max_velocity: f64,
    acceleration: f64,
    arc: Option<ArcMotion>,
}

impl ActiveMove {
    fn from_motion(m: Motion) -> Self {
        Self {
            start: m.start,
            target: m.target,
            total_distance: m.distance,
            travelled: 0.0,
            velocity: 0.0,
            max_velocity: m.max_velocity,
            acceleration: m.acceleration,
            arc: None,
        }
    }

    fn from_arc(
        start: [f64; 3],
        end: [f64; 3],
        center_x: f64,
        center_y: f64,
        radius: f64,
        start_angle: f64,
        sweep: f64,
        feed_mm_min: f64,
        config: &EmulatorConfig,
    ) -> Result<Self, WireError> {
        let planar_length = radius * sweep.abs();
        let dz = end[2] - start[2];
        let distance = (planar_length * planar_length + dz * dz).sqrt();
        if distance <= 1e-9 {
            return Err(api_error("bad_arc", "arc has zero length"));
        }

        // Ограничиваем скорость/ускорение по обеим осям XY и по Z при спирали.
        let mut max_velocity = (feed_mm_min / 60.0).max(0.001);
        let mut acceleration = f64::INFINITY;
        let tangent_x = planar_length.max(1e-9);
        let ratios = [
            (radius * sweep.abs() / tangent_x, Axis::X),
            (radius * sweep.abs() / tangent_x, Axis::Y),
            (dz.abs() / distance, Axis::Z),
        ];

        for (component, axis) in ratios {
            if component > 1e-9 {
                let limits = config.axis(axis);
                max_velocity = max_velocity.min(
                    limits.max_velocity_mm_min / 60.0 / component
                );
                acceleration = acceleration.min(
                    limits.acceleration_mm_s2 / component
                );
            }
        }

        if !acceleration.is_finite() {
            acceleration = config.axes.x.acceleration_mm_s2
                .min(config.axes.y.acceleration_mm_s2);
        }

        Ok(Self {
            start,
            target: end,
            total_distance: distance,
            travelled: 0.0,
            velocity: 0.0,
            max_velocity,
            acceleration,
            arc: Some(ArcMotion {
                center_x,
                center_y,
                radius,
                start_angle,
                sweep,
                start_z: start[2],
                end_z: end[2],
            }),
        })
    }

    fn tick(&mut self, dt: f64) -> ([f64; 3], bool) {
        let remaining = (self.total_distance - self.travelled).max(0.0);
        if remaining <= 1e-8 {
            return (self.target, true);
        }

        let brake_speed = (2.0 * self.acceleration * remaining).sqrt();
        let desired = self.max_velocity.min(brake_speed);

        if self.velocity < desired {
            self.velocity = (self.velocity + self.acceleration * dt).min(desired);
        } else {
            self.velocity = (self.velocity - self.acceleration * dt).max(desired);
        }

        let step = (self.velocity * dt).min(remaining);
        self.travelled += step;

        if self.total_distance - self.travelled <= 1e-8 {
            return (self.target, true);
        }

        let t = (self.travelled / self.total_distance).clamp(0.0, 1.0);

        if let Some(arc) = self.arc {
            let angle = arc.start_angle + arc.sweep * t;
            let position = [
                arc.center_x + arc.radius * angle.cos(),
                arc.center_y + arc.radius * angle.sin(),
                arc.start_z + (arc.end_z - arc.start_z) * t,
            ];
            (position, false)
        } else {
            (
                [
                    self.start[0] + (self.target[0] - self.start[0]) * t,
                    self.start[1] + (self.target[1] - self.start[1]) * t,
                    self.start[2] + (self.target[2] - self.start[2]) * t,
                ],
                false,
            )
        }
    }
}

struct Emulator {
    status: MachineStatus,
    config: EmulatorConfig,
    coordinate_offsets_path: PathBuf,
    program: Vec<String>,
    dwell_remaining: f64,
    absolute_mode: bool,
    active_move: Option<ActiveMove>,
    continuous_jog: Option<(Axis, i8, f64)>,
    homing_queue: Vec<Axis>,
    homing_axis: Option<Axis>,
    active_move_is_homing: bool,
    pending_spindle: Option<(SpindleDirection, f64)>,
    last_rapid_z: Option<f64>,
    stop_retract_active: bool,
    jog_velocity: f64,
    last_tick: Instant,
}

impl Emulator {
    fn new(config: EmulatorConfig, coordinate_offsets_path: PathBuf, offsets: Vec<CoordinateSystemOffset>) -> Self {
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

    fn select_coordinate_system(&mut self, system: CoordinateSystem) {
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

    fn persist_coordinate_offsets(&self) -> Result<(), WireError> {
        let yaml = serde_yaml_ng::to_string(&self.status.coordinate_systems)
            .map_err(|e| api_error("coordinates_save", e.to_string()))?;
        if let Some(parent) = self.coordinate_offsets_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| api_error("coordinates_save", e.to_string()))?;
        }
        fs::write(&self.coordinate_offsets_path, yaml)
            .map_err(|e| api_error("coordinates_save", e.to_string()))
    }

    fn position_array(&self) -> [f64; 3] {
        [self.status.position.x, self.status.position.y, self.status.position.z]
    }

    fn set_position(&mut self, p: [f64; 3]) {
        self.status.position = Position { x: p[0], y: p[1], z: p[2] };
    }

    fn check_target(&self, target: [f64; 3]) -> Result<(), WireError> {
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

    fn queue_move(&mut self, target: [f64; 3], feed: f64, rapid: bool) -> Result<(), WireError> {
        self.check_target(target)?;
        let motion = Motion::new(self.position_array(), target, feed, rapid, &self.config)?;
        self.active_move = motion.map(ActiveMove::from_motion);
        Ok(())
    }

    fn execute_gcode(&mut self, line: &str) -> Result<(), WireError> {
        let upper = line.to_ascii_uppercase();
        if upper.contains("G59.1") {
            self.select_coordinate_system(CoordinateSystem::G59_1);
        } else if upper.contains("G59.2") {
            self.select_coordinate_system(CoordinateSystem::G59_2);
        } else if upper.contains("G59.3") {
            self.select_coordinate_system(CoordinateSystem::G59_3);
        }

        let words = parse_words(line);
        let mut motion_code: Option<i32> = None;
        let mut dwell = false;
        let mut p_word: Option<f64> = None;
        let mut x_word = None;
        let mut y_word = None;
        let mut z_word = None;
        let mut i_word = None;
        let mut j_word = None;
        let mut l_word = None;

        for &(letter, value) in &words {
            match letter {
                'G' => {
                    // Десятичные варианты нужно распознать до преобразования в int:
                    // иначе G59.1 превратится в G59.
                    if (value - 59.1).abs() < 1e-6 {
                        self.select_coordinate_system(CoordinateSystem::G59_1);
                    } else if (value - 59.2).abs() < 1e-6 {
                        self.select_coordinate_system(CoordinateSystem::G59_2);
                    } else if (value - 59.3).abs() < 1e-6 {
                        self.select_coordinate_system(CoordinateSystem::G59_3);
                    } else {
                        match value as i32 {
                            0 | 1 | 2 | 3 => motion_code = Some(value as i32),
                            4 => dwell = true,
                            17 => {} // Поддерживаемая дуговая плоскость XY.
                            54 => self.select_coordinate_system(CoordinateSystem::G54),
                            55 => self.select_coordinate_system(CoordinateSystem::G55),
                            56 => self.select_coordinate_system(CoordinateSystem::G56),
                            57 => self.select_coordinate_system(CoordinateSystem::G57),
                            58 => self.select_coordinate_system(CoordinateSystem::G58),
                            59 => self.select_coordinate_system(CoordinateSystem::G59),
                            90 => self.absolute_mode = true,
                            91 => self.absolute_mode = false,
                            20 => return Err(api_error("unsupported_gcode", "G20 inch units are not supported; use G21")),
                            21 => {}
                            other => return Err(api_error("unsupported_gcode", format!("unsupported G-code G{other}"))),
                        }
                    }
                },
                'X' => x_word = Some(value),
                'Y' => y_word = Some(value),
                'Z' => z_word = Some(value),
                'I' => i_word = Some(value),
                'J' => j_word = Some(value),
                'P' => p_word = Some(value),
                'L' => l_word = Some(value),
                'F' if value.is_finite() && value > 0.0 => self.status.feed_rate = value,
                'F' => return Err(api_error("bad_feed", "F must be positive and finite")),
                'S' if value.is_finite() && value >= 0.0 => {
                    if value > self.config.spindle.max_rpm {
                        return Err(api_error("spindle_limit", format!(
                            "S{value:.0} exceeds maximum {:.0} RPM",
                            self.config.spindle.max_rpm
                        )));
                    }
                    self.status.spindle.commanded_rpm = value;
                }
                'M' => match value as i32 {
                    3 => self.set_spindle(SpindleDirection::Clockwise, None),
                    4 => self.set_spindle(SpindleDirection::CounterClockwise, None),
                    5 => self.set_spindle(SpindleDirection::Stopped, Some(0.0)),
                    7 => {
                        self.status.mist_coolant_on = true;
                        self.status.message = Some("Mist coolant ON (simulated)".into());
                    }
                    8 => {
                        self.status.flood_coolant_on = true;
                        self.status.message = Some("Flood coolant ON (simulated)".into());
                    }
                    9 => {
                        self.status.mist_coolant_on = false;
                        self.status.flood_coolant_on = false;
                        self.status.message = Some("Coolant OFF".into());
                    }
                    30 => {
                        self.status.message = Some("Program end M30".into());
                    }
                    2 => {
                        self.status.message = Some("Program end M2".into());
                    }
                    other => return Err(api_error("unsupported_gcode", format!("unsupported M-code M{other}"))),
                },
                // Стандартные слова, не используемые обработчиком траектории.
                'N' | 'T' | 'H' => {}
                _ => {}
            }
        }

        if words.iter().any(|(letter, value)| *letter == 'G' && (*value as i32) == 10) {
            if l_word.map(|v| v as i32) != Some(2) {
                return Err(api_error("unsupported_gcode", "only G10 L2 is supported"));
            }
            let p = p_word.ok_or_else(|| api_error("bad_g10", "G10 L2 requires P1..P9"))? as i32;
            let system = match p {
                1 => CoordinateSystem::G54,
                2 => CoordinateSystem::G55,
                3 => CoordinateSystem::G56,
                4 => CoordinateSystem::G57,
                5 => CoordinateSystem::G58,
                6 => CoordinateSystem::G59,
                7 => CoordinateSystem::G59_1,
                8 => CoordinateSystem::G59_2,
                9 => CoordinateSystem::G59_3,
                _ => return Err(api_error("bad_g10", "G10 L2 P must be in range 1..9")),
            };

            let mut offset = self.status.coordinate_systems.iter()
                .find(|entry| entry.system == system)
                .map(|entry| entry.offset)
                .unwrap_or(Position { x: 0.0, y: 0.0, z: 0.0 });

            if let Some(x) = x_word { offset.x = x; }
            if let Some(y) = y_word { offset.y = y; }
            if let Some(z) = z_word { offset.z = z; }

            if let Some(entry) = self.status.coordinate_systems.iter_mut()
                .find(|entry| entry.system == system)
            {
                entry.offset = offset;
            }
            self.persist_coordinate_offsets()?;
            self.status.message = Some(format!("G10 L2 {} offset saved", system.label()));
            return Ok(());
        }

        if dwell {
            let seconds = p_word
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| api_error("bad_dwell", "G4 requires a finite, non-negative P value in seconds"))?;

            self.dwell_remaining = seconds;
            self.status.message = Some(format!("Dwell: {:.3} s", seconds));
            return Ok(());
        }

        if let Some(code) = motion_code {
            let current = self.position_array();
            let offset = [
                self.status.work_offset.x,
                self.status.work_offset.y,
                self.status.work_offset.z,
            ];
            let words_axis = [x_word, y_word, z_word];
            let mut target = current;

            for index in 0..3 {
                if let Some(value) = words_axis[index] {
                    target[index] = if self.absolute_mode {
                        value + offset[index]
                    } else {
                        current[index] + value
                    };
                }
            }

            self.check_target(target)?;

            if code == 2 || code == 3 {
                if i_word.is_none() && j_word.is_none() {
                    return Err(api_error("bad_arc", "G2/G3 requires I or J center offsets"));
                }

                let center_x = current[0] + i_word.unwrap_or(0.0);
                let center_y = current[1] + j_word.unwrap_or(0.0);
                let radius_start = ((current[0] - center_x).powi(2)
                    + (current[1] - center_y).powi(2)).sqrt();
                let radius_end = ((target[0] - center_x).powi(2)
                    + (target[1] - center_y).powi(2)).sqrt();

                if radius_start <= 1e-9
                    || (radius_start - radius_end).abs() > (0.02_f64).max(radius_start * 0.002)
                {
                    return Err(api_error("bad_arc", format!(
                        "arc radii mismatch: start={radius_start:.4}, end={radius_end:.4}"
                    )));
                }

                let start_angle = (current[1] - center_y).atan2(current[0] - center_x);
                let end_angle = (target[1] - center_y).atan2(target[0] - center_x);
                let same_xy = (target[0] - current[0]).abs() < 1e-8
                    && (target[1] - current[1]).abs() < 1e-8;

                let sweep = if same_xy {
                    if code == 2 { -2.0 * PI } else { 2.0 * PI }
                } else if code == 2 {
                    let mut a = end_angle - start_angle;
                    while a >= 0.0 { a -= 2.0 * PI; }
                    a
                } else {
                    let mut a = end_angle - start_angle;
                    while a <= 0.0 { a += 2.0 * PI; }
                    a
                };

                // Проверяем мягкие пределы по дуге, а не только в конечной точке.
                let samples = ((radius_start * sweep.abs() / 1.0).ceil() as usize).max(16);
                for n in 0..=samples {
                    let t = n as f64 / samples as f64;
                    let angle = start_angle + sweep * t;
                    let p = [
                        center_x + radius_start * angle.cos(),
                        center_y + radius_start * angle.sin(),
                        current[2] + (target[2] - current[2]) * t,
                    ];
                    self.check_target(p)?;
                }

                if self.active_move.is_some() {
                    return Err(api_error("motion_busy", "another motion is active"));
                }

                let arc = ActiveMove::from_arc(
                    current,
                    target,
                    center_x,
                    center_y,
                    radius_start,
                    start_angle,
                    sweep,
                    self.status.feed_rate,
                    &self.config,
                )?;
                self.active_move = Some(arc);
            } else if target != current {
                if code == 0 && z_word.is_some() {
                    self.last_rapid_z = Some(target[2]);
                }
                if self.active_move.is_some() {
                    return Err(api_error("motion_busy", "another motion is active"));
                }
                let rapid = code == 0;
                let feed = if rapid {
                    self.config.axes.x.max_velocity_mm_min
                        .min(self.config.axes.y.max_velocity_mm_min)
                        .min(self.config.axes.z.max_velocity_mm_min)
                } else {
                    self.status.feed_rate
                };
                self.queue_move(target, feed, rapid)?;
            }
        } else if x_word.is_some() || y_word.is_some() || z_word.is_some() {
            return Err(api_error("unsupported_gcode", "axis words require an explicit supported motion G0/G1/G2/G3"));
        }

        self.status.message = Some(format!("Executing: {line}"));
        Ok(())
    }

    fn set_spindle(&mut self, direction: SpindleDirection, rpm: Option<f64>) {
        let requested_rpm = rpm.unwrap_or(self.status.spindle.commanded_rpm)
            .clamp(0.0, self.config.spindle.max_rpm);

        if direction == SpindleDirection::Stopped || requested_rpm <= 0.0 {
            // STOP отменяет ожидающее реверсирование и начинает торможение.
            self.pending_spindle = None;
            self.status.spindle.commanded_rpm = 0.0;

            if self.status.spindle.actual_rpm <= 0.01 {
                self.status.spindle.actual_rpm = 0.0;
                self.status.spindle.direction = SpindleDirection::Stopped;
                self.status.spindle.enabled = false;
            } else {
                self.status.spindle.enabled = true;
            }
            return;
        }

        let current_direction = self.status.spindle.direction;
        let is_running = self.status.spindle.actual_rpm > 0.01;

        if is_running && current_direction != direction {
            // Сначала тормозим до нуля. Tick запустит новый разгон после остановки.
            self.pending_spindle = Some((direction, requested_rpm));
            self.status.spindle.commanded_rpm = 0.0;
            self.status.spindle.enabled = true;
        } else {
            self.pending_spindle = None;
            self.status.spindle.direction = direction;
            self.status.spindle.commanded_rpm = requested_rpm;
            self.status.spindle.enabled = true;
        }
    }

    fn handle(&mut self, request: Request) -> Result<Value, WireError> {
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

    fn tick(&mut self, dt: f64) {
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

fn api_error(code: impl Into<String>, message: impl Into<String>) -> WireError {
    WireError { code: code.into(), message: message.into() }
}

fn parse_words(line: &str) -> Vec<(char, f64)> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_alphabetic() { i += 1; continue; }
        let letter = chars[i].to_ascii_uppercase();
        i += 1;
        let start = i;
        if i < chars.len() && matches!(chars[i], '+' | '-') { i += 1; }
        let mut dot_seen = false;
        while i < chars.len() {
            if chars[i].is_ascii_digit() { i += 1; }
            else if chars[i] == '.' && !dot_seen { dot_seen = true; i += 1; }
            else { break; }
        }
        if i > start {
            let value: String = chars[start..i].iter().collect();
            if let Ok(value) = value.parse::<f64>() { words.push((letter, value)); }
        }
    }
    words
}

fn write_response(stream: &mut UnixStream, result: Result<Value, WireError>) -> std::io::Result<()> {
    let response = match result {
        Ok(result) => Response { ok: true, result, error: None },
        Err(error) => Response { ok: false, result: Value::Null, error: Some(error) },
    };
    serde_json::to_writer(&mut *stream, &response)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn serve_client(stream: UnixStream, state: Arc<Mutex<Emulator>>) {
    let reader_stream = match stream.try_clone() { Ok(s) => s, Err(e) => { eprintln!("{e}"); return; } };
    let mut reader = BufReader::new(reader_stream);
    let mut writer = stream;

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => { eprintln!("Socket read error: {e}"); return; }
        }

        let parsed = serde_json::from_str::<Request>(&line)
            .map_err(|e| api_error("bad_json", e.to_string()));

        let result = match parsed {
            Ok(request) => match state.lock() {
                Ok(mut emulator) => emulator.handle(request),
                Err(_) => Err(api_error("internal", "state lock poisoned")),
            },
            Err(error) => Err(error),
        };

        if let Err(error) = write_response(&mut writer, result) {
            eprintln!("Socket write error: {error}");
            return;
        }
    }
}

fn default_coordinate_offsets() -> Vec<CoordinateSystemOffset> {
    CoordinateSystem::ALL.iter().map(|system| CoordinateSystemOffset {
        system: *system,
        offset: Position { x: 0.0, y: 0.0, z: 0.0 },
    }).collect()
}

fn load_coordinate_offsets(path: &Path) -> Result<Vec<CoordinateSystemOffset>, String> {
    if !path.exists() {
        return Ok(default_coordinate_offsets());
    }
    let content = fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut offsets: Vec<CoordinateSystemOffset> = serde_yaml_ng::from_str(&content)
        .map_err(|e| format!("invalid coordinate offsets {}: {e}", path.display()))?;

    for system in CoordinateSystem::ALL {
        if !offsets.iter().any(|entry| entry.system == system) {
            offsets.push(CoordinateSystemOffset {
                system,
                offset: Position { x: 0.0, y: 0.0, z: 0.0 },
            });
        }
    }
    Ok(offsets)
}

fn load_config(path: &Path) -> Result<EmulatorConfig, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let config: EmulatorConfig = serde_yaml_ng::from_str(&content)
        .map_err(|e| format!("invalid YAML {}: {e}", path.display()))?;
    config.validate()?;
    Ok(config)
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let socket_path = args.next().map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/linuxcnc-ui-emulator.sock"));
    let config_path = args.next().map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/emulator.yaml"));

    let config = load_config(&config_path).unwrap_or_else(|e| {
        eprintln!("Configuration error: {e}");
        std::process::exit(2);
    });
    let offsets_path = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("coordinate_systems.yaml");
    let coordinate_offsets = load_coordinate_offsets(&offsets_path).unwrap_or_else(|e| {
        eprintln!("Coordinate-system storage error: {e}");
        std::process::exit(2);
    });

    if socket_path.exists() {
        match fs::symlink_metadata(&socket_path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                if UnixStream::connect(&socket_path).is_ok() {
                    eprintln!("An emulator already listens on {}", socket_path.display());
                    std::process::exit(1);
                }
                if let Err(e) = fs::remove_file(&socket_path) {
                    eprintln!("Cannot remove stale socket: {e}");
                    std::process::exit(1);
                }
            }
            Ok(_) => {
                eprintln!("Refusing to replace non-socket path {}", socket_path.display());
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Cannot inspect socket path: {e}");
                std::process::exit(1);
            }
        }
    }

    let listener = UnixListener::bind(&socket_path).unwrap_or_else(|e| {
        eprintln!("Cannot bind {}: {e}", socket_path.display());
        std::process::exit(1);
    });

    println!("Loaded configuration from {}", config_path.display());
    println!("CNC emulator listening on {}", socket_path.display());

    let state = Arc::new(Mutex::new(Emulator::new(config, offsets_path, coordinate_offsets)));
    let ticker = state.clone();
    thread::spawn(move || {
        let period = Duration::from_millis(10);
        loop {
            thread::sleep(period);
            if let Ok(mut emulator) = ticker.lock() {
                let now = Instant::now();
                let dt = now.duration_since(emulator.last_tick).as_secs_f64().clamp(0.001, 0.05);
                emulator.last_tick = now;
                emulator.tick(dt);
            }
        }
    });

    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                let state = state.clone();
                thread::spawn(move || serve_client(stream, state));
            }
            Err(e) => eprintln!("Accept error: {e}"),
        }
    }
}
