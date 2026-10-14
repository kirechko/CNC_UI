 //! Draws the orientation cube and maps pointer clicks to visible faces.
use super::types::Camera;

const CUBE_VERTICES: [[f64; 3]; 8] = [
    [-1.0, -1.0, -1.0],
    [1.0, -1.0, -1.0],
    [1.0, 1.0, -1.0],
    [-1.0, 1.0, -1.0],
    [-1.0, -1.0, 1.0],
    [1.0, -1.0, 1.0],
    [1.0, 1.0, 1.0],
    [-1.0, 1.0, 1.0],
];

const CUBE_FACES: [([usize; 4], [f64; 3], &str, (f64, f64, f64)); 6] = [
    ([1, 2, 6, 5], [1.0, 0.0, 0.0], "Right", (0.50, 0.52, 0.56)),
    ([0, 4, 7, 3], [-1.0, 0.0, 0.0], "Left", (0.42, 0.44, 0.48)),
    ([3, 7, 6, 2], [0.0, 1.0, 0.0], "Top", (0.66, 0.67, 0.70)),
    ([0, 1, 5, 4], [0.0, -1.0, 0.0], "Bottom", (0.36, 0.38, 0.42)),
    ([4, 5, 6, 7], [0.0, 0.0, 1.0], "Front", (0.56, 0.58, 0.61)),
    ([0, 3, 2, 1], [0.0, 0.0, -1.0], "Back", (0.46, 0.48, 0.51)),
];

fn cube_points(camera: &Camera, width: f64, height: f64) -> Vec<(f64, f64, f64)> {
    let cx = width / 2.0;
    let cy = height / 2.0;
    let size = width.min(height) * 0.23;

    CUBE_VERTICES
        .iter()
        .map(|v| {
            let x = v[0] * camera.orientation.right[0]
                + v[1] * camera.orientation.right[1]
                + v[2] * camera.orientation.right[2];
            let y = v[0] * camera.orientation.up[0]
                + v[1] * camera.orientation.up[1]
                + v[2] * camera.orientation.up[2];
            let z = v[0] * camera.orientation.fwd[0]
                + v[1] * camera.orientation.fwd[1]
                + v[2] * camera.orientation.fwd[2];
            (cx + x * size, cy - y * size, z)
        })
        .collect()
}

fn rounded_polygon_path(cr: &gtk4::cairo::Context, points: &[(f64, f64)], radius: f64) {
    if points.len() < 3 {
        return;
    }

    let mut incoming = Vec::with_capacity(points.len());
    let mut outgoing = Vec::with_capacity(points.len());

    for i in 0..points.len() {
        let previous = points[(i + points.len() - 1) % points.len()];
        let current = points[i];
        let next = points[(i + 1) % points.len()];

        let to_previous = (previous.0 - current.0, previous.1 - current.1);
        let to_next = (next.0 - current.0, next.1 - current.1);

        let len_previous = (to_previous.0 * to_previous.0 + to_previous.1 * to_previous.1).sqrt();
        let len_next = (to_next.0 * to_next.0 + to_next.1 * to_next.1).sqrt();

        if len_previous < 1e-6 || len_next < 1e-6 {
            incoming.push(current);
            outgoing.push(current);
            continue;
        }

        let trim = radius.min(len_previous * 0.25).min(len_next * 0.25);

        incoming.push((
            current.0 + to_previous.0 / len_previous * trim,
            current.1 + to_previous.1 / len_previous * trim,
        ));
        outgoing.push((
            current.0 + to_next.0 / len_next * trim,
            current.1 + to_next.1 / len_next * trim,
        ));
    }

    cr.move_to(outgoing[0].0, outgoing[0].1);

    for i in 1..points.len() {
        cr.line_to(incoming[i].0, incoming[i].1);
        cr.curve_to(
            points[i].0,
            points[i].1,
            points[i].0,
            points[i].1,
            outgoing[i].0,
            outgoing[i].1,
        );
    }

    cr.line_to(incoming[0].0, incoming[0].1);
    cr.curve_to(
        points[0].0,
        points[0].1,
        points[0].0,
        points[0].1,
        outgoing[0].0,
        outgoing[0].1,
    );
    cr.close_path();
}

pub(super) fn draw_orientation_cube(
    cr: &gtk4::cairo::Context,
    width: f64,
    height: f64,
    camera: &Camera,
) {
    let points = cube_points(camera, width, height);
    let mut faces = Vec::new();

    for (indices, normal, label, color) in CUBE_FACES {
        let facing = normal[0] * camera.orientation.fwd[0]
            + normal[1] * camera.orientation.fwd[1]
            + normal[2] * camera.orientation.fwd[2];

        if facing > 1e-8 {
            let depth = indices.iter().map(|i| points[*i].2).sum::<f64>() / 4.0;
            faces.push((depth, facing, indices, label, color));
        }
    }

    faces.sort_by(|a, b| a.0.total_cmp(&b.0));

    for (_, facing, indices, label, color) in faces {
        let polygon: Vec<(f64, f64)> = indices
            .iter()
            .map(|index| (points[*index].0, points[*index].1))
            .collect();

        rounded_polygon_path(cr, &polygon, 80.0);
        cr.set_source_rgb(color.0, color.1, color.2);
        cr.fill_preserve().unwrap();

        // Тёмная тонкая кромка и мягкий блик имитируют фаску.
        cr.set_line_width(2.0);
        cr.set_line_join(gtk4::cairo::LineJoin::Round);
        cr.set_source_rgb(0.20, 0.21, 0.23);
        cr.stroke_preserve().unwrap();

        cr.set_line_width(0.8);
        cr.set_source_rgba(0.92, 0.94, 0.98, 0.55);
        cr.stroke().unwrap();

        let cx = indices.iter().map(|i| points[*i].0).sum::<f64>() / 4.0;
        let cy = indices.iter().map(|i| points[*i].1).sum::<f64>() / 4.0;

        // Подписи — только на хорошо видимых гранях.
        if facing >= 0.5 {
            cr.select_font_face(
                "Sans",
                gtk4::cairo::FontSlant::Normal,
                gtk4::cairo::FontWeight::Bold,
            );
            cr.set_source_rgb(0.1, 0.1, 0.12);

            // Берём самое длинное ребро, чей наклон позволяет оставить
            // надпись читаемой и не перевёрнутой: от -45° до +45°.
            let mut chosen: Option<(f64, f64)> = None; // (длина, угол)
            let mut fallback: Option<(f64, f64)> = None;

            for edge in 0..indices.len() {
                let a = points[indices[edge]];
                let b = points[indices[(edge + 1) % indices.len()]];
                let dx = b.0 - a.0;
                let dy = b.1 - a.1;
                let length = (dx * dx + dy * dy).sqrt();

                if length < 1.0 {
                    continue;
                }

                let mut angle = dy.atan2(dx);
                while angle > std::f64::consts::FRAC_PI_2 {
                    angle -= std::f64::consts::PI;
                }
                while angle < -std::f64::consts::FRAC_PI_2 {
                    angle += std::f64::consts::PI;
                }

                if fallback.map_or(true, |(best, _)| length > best) {
                    fallback = Some((length, angle));
                }

                if angle.abs() <= std::f64::consts::FRAC_PI_4
                    && chosen.map_or(true, |(best, _)| length > best)
                {
                    chosen = Some((length, angle));
                }
            }

            if let Some((edge_length, angle)) = chosen.or(fallback) {
                // Сужающиеся грани получают меньший шрифт, чтобы надпись
                // не налезала на соседнюю грань или её общее ребро.
                cr.set_font_size(11.0);
                let initial = cr.text_extents(label).unwrap();
                let available_width = edge_length * 0.68;
                let font_size =
                    (11.0 * available_width / initial.width().max(1.0)).clamp(6.0, 11.0);
                cr.set_font_size(font_size);

                let extents = cr.text_extents(label).unwrap();
                cr.save().unwrap();
                cr.translate(cx, cy);
                cr.rotate(angle);
                cr.move_to(
                    -extents.x_bearing() - extents.width() / 2.0,
                    -extents.y_bearing() - extents.height() / 2.0,
                );
                cr.show_text(label).unwrap();
                cr.restore().unwrap();
            }
        }
    }
}

pub(super) fn cube_face_at(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    camera: &Camera,
) -> Option<[f64; 3]> {
    let points = cube_points(camera, width, height);
    let mut candidates = Vec::new();

    for (indices, normal, _, _) in CUBE_FACES {
        let facing = normal[0] * camera.orientation.fwd[0]
            + normal[1] * camera.orientation.fwd[1]
            + normal[2] * camera.orientation.fwd[2];

        if facing <= 1e-8 {
            continue;
        }

        let polygon: Vec<(f64, f64)> = indices
            .iter()
            .map(|i| (points[*i].0, points[*i].1))
            .collect();

        if point_in_polygon(x, y, &polygon) {
            let depth = indices.iter().map(|i| points[*i].2).sum::<f64>() / 4.0;
            candidates.push((depth, normal));
        }
    }

    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    candidates.first().map(|(_, normal)| *normal)
}

fn point_in_polygon(x: f64, y: f64, polygon: &[(f64, f64)]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;

    for i in 0..polygon.len() {
        let (xi, yi) = polygon[i];
        let (xj, yj) = polygon[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}
