 //! Converts supported G-code moves into renderable toolpath segments and bounds.
use super::types::{MoveType, Segment};
use super::GCodeViewer;

impl GCodeViewer {
    pub(super) fn parse_gcode(&mut self, content: &str) {
        let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
        let (mut last_i, mut last_j) = (0.0, 0.0);
        let mut last_move = MoveType::Rapid;

        for line in content.lines() {
            let code = line.split(';').next().unwrap_or("").trim();
            if code.is_empty() {
                continue;
            }

            let (start_x, start_y, start_z) = (x, y, z);
            let (mut nx, mut ny, mut nz) = (None, None, None);
            let (mut ni, mut nj, mut mt) = (None, None, None);

            for token in code.split_whitespace() {
                if token.len() < 2 {
                    continue;
                }
                let Ok(value) = token[1..].parse::<f64>() else {
                    continue;
                };
                match &token[..1] {
                    "G" => match value as i32 {
                        0 => mt = Some(MoveType::Rapid),
                        1 => mt = Some(MoveType::Linear),
                        2 => mt = Some(MoveType::ArcClockwise),
                        3 => mt = Some(MoveType::ArcCounterClockwise),
                        _ => {}
                    },
                    "X" => nx = Some(value),
                    "Y" => ny = Some(value),
                    "Z" => nz = Some(value),
                    "I" => ni = Some(value),
                    "J" => nj = Some(value),
                    _ => {}
                }
            }

            if let Some(v) = ni {
                last_i = v;
            }
            if let Some(v) = nj {
                last_j = v;
            }

            let move_type = mt.unwrap_or(last_move);
            last_move = move_type;

            let end_x = nx.unwrap_or(start_x);
            let end_y = ny.unwrap_or(start_y);
            let end_z = nz.unwrap_or(start_z);

            if matches!(
                move_type,
                MoveType::ArcClockwise | MoveType::ArcCounterClockwise
            ) && (nx.is_some() || ny.is_some() || ni.is_some() || nj.is_some())
            {
                let cx = start_x + last_i;
                let cy = start_y + last_j;
                let radius = (last_i * last_i + last_j * last_j).sqrt();
                let start_angle = (start_y - cy).atan2(start_x - cx);
                let end_angle = (end_y - cy).atan2(end_x - cx);
                let same_xy = (end_x - start_x).abs() < 1e-8 && (end_y - start_y).abs() < 1e-8;

                // G2 — clockwise (отрицательный sweep), G3 — counter-clockwise.
                let sweep = match move_type {
                    MoveType::ArcClockwise if same_xy => -2.0 * std::f64::consts::PI,
                    MoveType::ArcCounterClockwise if same_xy => 2.0 * std::f64::consts::PI,
                    MoveType::ArcClockwise => {
                        let mut a = end_angle - start_angle;
                        while a >= 0.0 {
                            a -= 2.0 * std::f64::consts::PI;
                        }
                        a
                    }
                    MoveType::ArcCounterClockwise => {
                        let mut a = end_angle - start_angle;
                        while a <= 0.0 {
                            a += 2.0 * std::f64::consts::PI;
                        }
                        a
                    }
                    _ => unreachable!("arc branch only handles G2/G3"),
                };

                let steps = (sweep.abs() / 0.08).ceil().max(4.0) as usize;
                let mut previous = (start_x, start_y, start_z);

                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    let angle = start_angle + sweep * t;
                    let next = (
                        cx + radius * angle.cos(),
                        cy + radius * angle.sin(),
                        start_z + (end_z - start_z) * t,
                    );
                    self.segments.push(Segment {
                        x_start: previous.0,
                        y_start: previous.1,
                        z_start: previous.2,
                        x_end: next.0,
                        y_end: next.1,
                        z_end: next.2,
                        move_type,
                    });
                    previous = next;
                }
            } else if nx.is_some() || ny.is_some() || nz.is_some() {
                self.segments.push(Segment {
                    x_start: start_x,
                    y_start: start_y,
                    z_start: start_z,
                    x_end: end_x,
                    y_end: end_y,
                    z_end: end_z,
                    move_type,
                });
            }

            x = end_x;
            y = end_y;
            z = end_z;
        }
    }

    pub(super) fn calculate_bounds(&mut self) {
        if self.segments.is_empty() {
            return;
        }

        let mut min = (f64::MAX, f64::MAX, f64::MAX);
        let mut max = (f64::MIN, f64::MIN, f64::MIN);

        for s in &self.segments {
            min.0 = min.0.min(s.x_start).min(s.x_end);
            max.0 = max.0.max(s.x_start).max(s.x_end);
            min.1 = min.1.min(s.y_start).min(s.y_end);
            max.1 = max.1.max(s.y_start).max(s.y_end);
            min.2 = min.2.min(s.z_start).min(s.z_end);
            max.2 = max.2.max(s.z_start).max(s.z_end);
        }

        self.bounds = (min.0, min.1, min.2, max.0, max.1, max.2);
    }
}
