use super::super::dynamics::ActiveMove;
use super::super::protocol::{api_error, WireError};
use super::super::state::Emulator;
use std::f64::consts::PI;

pub(super) fn execute_motion(
    emulator: &mut Emulator,
    code: i32,
    x_word: Option<f64>,
    y_word: Option<f64>,
    z_word: Option<f64>,
    i_word: Option<f64>,
    j_word: Option<f64>,
) -> Result<(), WireError> {
    let current = emulator.position_array();
    let offset = [
        emulator.status.work_offset.x,
        emulator.status.work_offset.y,
        emulator.status.work_offset.z,
    ];
    let words_axis = [x_word, y_word, z_word];
    let mut target = current;

    for index in 0..3 {
        if let Some(value) = words_axis[index] {
            target[index] = if emulator.absolute_mode {
                value + offset[index]
            } else {
                current[index] + value
            };
        }
    }

    emulator.check_target(target)?;

    if code == 2 || code == 3 {
        if i_word.is_none() && j_word.is_none() {
            return Err(api_error("bad_arc", "G2/G3 requires I or J center offsets"));
        }

        let center_x = current[0] + i_word.unwrap_or(0.0);
        let center_y = current[1] + j_word.unwrap_or(0.0);
        let radius_start =
            ((current[0] - center_x).powi(2) + (current[1] - center_y).powi(2)).sqrt();
        let radius_end =
            ((target[0] - center_x).powi(2) + (target[1] - center_y).powi(2)).sqrt();

        if radius_start <= 1e-9
            || (radius_start - radius_end).abs() > (0.02_f64).max(radius_start * 0.002)
        {
            return Err(api_error(
                "bad_arc",
                format!("arc radii mismatch: start={radius_start:.4}, end={radius_end:.4}"),
            ));
        }

        let start_angle = (current[1] - center_y).atan2(current[0] - center_x);
        let end_angle = (target[1] - center_y).atan2(target[0] - center_x);
        let same_xy = (target[0] - current[0]).abs() < 1e-8
            && (target[1] - current[1]).abs() < 1e-8;

        let sweep = if same_xy {
            if code == 2 {
                -2.0 * PI
            } else {
                2.0 * PI
            }
        } else if code == 2 {
            let mut angle = end_angle - start_angle;
            while angle >= 0.0 {
                angle -= 2.0 * PI;
            }
            angle
        } else {
            let mut angle = end_angle - start_angle;
            while angle <= 0.0 {
                angle += 2.0 * PI;
            }
            angle
        };

        let samples = ((radius_start * sweep.abs() / 1.0).ceil() as usize).max(16);
        for sample in 0..=samples {
            let t = sample as f64 / samples as f64;
            let angle = start_angle + sweep * t;
            emulator.check_target([
                center_x + radius_start * angle.cos(),
                center_y + radius_start * angle.sin(),
                current[2] + (target[2] - current[2]) * t,
            ])?;
        }

        if emulator.active_move.is_some() {
            return Err(api_error("motion_busy", "another motion is active"));
        }

        emulator.active_move = Some(ActiveMove::from_arc(
            current,
            target,
            center_x,
            center_y,
            radius_start,
            start_angle,
            sweep,
            emulator.status.feed_rate,
            &emulator.config,
        )?);
    } else if target != current {
        if code == 0 && z_word.is_some() {
            emulator.last_rapid_z = Some(target[2]);
        }
        if emulator.active_move.is_some() {
            return Err(api_error("motion_busy", "another motion is active"));
        }

        let rapid = code == 0;
        let feed = if rapid {
            emulator
                .config
                .axes
                .x
                .max_velocity_mm_min
                .min(emulator.config.axes.y.max_velocity_mm_min)
                .min(emulator.config.axes.z.max_velocity_mm_min)
        } else {
            emulator.status.feed_rate
        };
        emulator.queue_move(target, feed, rapid)?;
    }

    Ok(())
}
