use crate::cnc_api::Axis;
use crate::config::EmulatorConfig;
use crate::protocol::{api_error, WireError};

#[derive(Clone, Copy)]
pub(super) struct Motion {
    start: [f64; 3],
    target: [f64; 3],
    distance: f64,
    max_velocity: f64,    // mm/s along path
    acceleration: f64,   // mm/s^2 along path
}

impl Motion {
    pub(super) fn new(
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

pub(super) struct ActiveMove {
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

    pub(super) fn tick(&mut self, dt: f64) -> ([f64; 3], bool) {
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
