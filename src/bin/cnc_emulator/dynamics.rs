 //! Calculates constrained moves and advances their simulated motion over time.
use crate::cnc_api::Axis;
use crate::config::EmulatorConfig;
use crate::protocol::{api_error, WireError};

#[derive(Clone, Copy)]
pub(super) struct Motion {
    start: [f64; 5],
    target: [f64; 5],
    distance: f64,
    max_velocity: f64, // mm/s along path
    acceleration: f64, // mm/s^2 along path
}

impl Motion {
    pub(super) fn new(
        start: [f64; 5],
        target: [f64; 5],
        feed_mm_min: f64,
        rapid: bool,
        config: &EmulatorConfig,
    ) -> Result<Option<Self>, WireError> {
        let delta = std::array::from_fn::<_, 5, _>(|index| target[index] - start[index]);
        let distance = delta.iter().map(|component| component * component).sum::<f64>().sqrt();
        if distance < 1e-9 {
            return Ok(None);
        }

        let mut max_velocity = if rapid {
            f64::INFINITY
        } else {
            (feed_mm_min / 60.0).max(0.001)
        };
        let mut acceleration = f64::INFINITY;

        for (index, axis) in config.configured_axes().into_iter().enumerate() {
            let component = (delta[index] / distance).abs();
            if component > 1e-9 {
                let (_, _, velocity, axis_acceleration) = config.axis_limits(axis);
                max_velocity = max_velocity.min(velocity / 60.0 / component);
                acceleration = acceleration.min(axis_acceleration / component);
            }
        }

        if !max_velocity.is_finite() || !acceleration.is_finite() {
            return Err(api_error(
                "motion_config",
                "could not calculate motion limits",
            ));
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

pub(super) struct ActiveMove {
    start: [f64; 5],
    target: [f64; 5],
    total_distance: f64,
    travelled: f64,
    velocity: f64,
    max_velocity: f64,
    acceleration: f64,
    arc: Option<ArcMotion>,
}

impl ActiveMove {
    pub(super) fn from_motion(m: Motion) -> Self {
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

    pub(super) fn from_arc(
        start: [f64; 5],
        end: [f64; 5],
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
                let (_, _, velocity, axis_acceleration) = config.axis_limits(axis);
                max_velocity = max_velocity.min(velocity / 60.0 / component);
                acceleration = acceleration.min(axis_acceleration / component);
            }
        }

        if !acceleration.is_finite() {
            acceleration = config.axes.x.acceleration_mm_s2.min(config.axes.y.acceleration_mm_s2);
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

    pub(super) fn velocity_mm_min(&self) -> f64 {
        self.velocity * 60.0
    }

    pub(super) fn tick(&mut self, dt: f64) -> ([f64; 5], bool) {
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
            let mut position = std::array::from_fn(|index| {
                self.start[index] + (self.target[index] - self.start[index]) * t
            });
            position[0] = arc.center_x + arc.radius * angle.cos();
            position[1] = arc.center_y + arc.radius * angle.sin();
            position[2] = arc.start_z + (arc.end_z - arc.start_z) * t;
            (position, false)
        } else {
            (
                std::array::from_fn(|index| {
                    self.start[index] + (self.target[index] - self.start[index]) * t
                }),
                false,
            )
        }
    }
}
