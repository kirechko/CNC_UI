 //! Draws the spindle tool at its reported position and rotation phase.
use super::types::{Camera, ToolVisual};
use std::f64::consts::PI;

pub(super) fn draw_hex_pencil<F>(
    cr: &gtk4::cairo::Context,
    project: F,
    camera: &Camera,
    tool: &ToolVisual,
) where
    F: Fn(f64, f64, f64) -> (f64, f64),
{
    const SIDES: usize = 6;
    let radius = 1.5; // Диаметр карандаша 3 мм.
    let tip_z = tool.z;
    let shoulder_z = tip_z + 5.0;
    let body_top_z = tip_z + 22.0;
    let phase = tool.phase;

    // Это общее кольцо: корпус и заточенный кончик стыкуются без зазора.
    let ring: Vec<[f64; 3]> = (0..SIDES)
        .map(|i| {
            let angle = phase + i as f64 * 2.0 * PI / SIDES as f64;
            [
                tool.x + radius * angle.cos(),
                tool.y + radius * angle.sin(),
                shoulder_z,
            ]
        })
        .collect();

    let top_ring: Vec<[f64; 3]> = ring.iter().map(|p| [p[0], p[1], body_top_z]).collect();

    let tip = [tool.x, tool.y, tip_z];

    // Каждая грань хранит среднюю глубину и экранный полигон.
    // Сначала рисуются дальние грани, затем ближние.
    let mut faces: Vec<(f64, Vec<(f64, f64)>, (f64, f64, f64))> = Vec::new();

    let camera_depth = |p: [f64; 3]| {
        camera.orientation.fwd[0] * p[0]
            + camera.orientation.fwd[1] * p[1]
            + camera.orientation.fwd[2] * p[2]
    };

    // На остановке корпус темнее; при вращении появляется движущийся блик.
    // Яркость переключается только по факту вращения, не зависит от RPM.
    let brightness: f64 = if tool.rpm > 0.01 { 1.0 } else { 0.35 };

    for i in 0..SIDES {
        let j = (i + 1) % SIDES;

        // Боковая грань корпуса.
        let body_points = [ring[i], ring[j], top_ring[j], top_ring[i]];
        let body_depth = body_points.iter().map(|p| camera_depth(*p)).sum::<f64>() / 4.0;
        let body_poly = body_points
            .iter()
            .map(|p| project(p[0], p[1], p[2]))
            .collect();
        // Нейтральные серебристые оттенки стали подчёркивают грани корпуса.
        let base_color = match i {
            0 => (0.78, 0.80, 0.82),
            1 => (0.66, 0.68, 0.70),
            2 => (0.48, 0.50, 0.52),
            3 => (0.74, 0.76, 0.78),
            4 => (0.58, 0.60, 0.62),
            _ => (0.42, 0.44, 0.46),
        };

        let body_color = (
            (base_color.0 * brightness).min(1.0),
            (base_color.1 * brightness).min(1.0),
            (base_color.2 * brightness).min(1.0),
        );
        faces.push((body_depth, body_poly, body_color));

        // Треугольная заточка использует те же вершины кольца — щели быть не может.
        let tip_points = [tip, ring[i], ring[j]];
        let tip_depth = tip_points.iter().map(|p| camera_depth(*p)).sum::<f64>() / 3.0;
        let tip_poly = tip_points
            .iter()
            .map(|p| project(p[0], p[1], p[2]))
            .collect();
        let tip_shade: f64 = if i % 2 == 0 { 0.78 } else { 0.55 };
        let tip_brightness = brightness;
        let steel_shade = 0.72 * tip_shade * tip_brightness;
        faces.push((
            tip_depth,
            tip_poly,
            (steel_shade, steel_shade, steel_shade),
        ));
    }

    // Закрываем верх корпуса шестигранной крышкой.
    let top_depth = top_ring.iter().map(|p| camera_depth(*p)).sum::<f64>() / SIDES as f64;
    let top_poly = top_ring.iter().map(|p| project(p[0], p[1], p[2])).collect();
    let top_shade = 0.76 * brightness;
    faces.push((top_depth, top_poly, (top_shade, top_shade, top_shade)));

    faces.sort_by(|a, b| a.0.total_cmp(&b.0));

    for (_, polygon, color) in faces {
        if polygon.len() < 3 {
            continue;
        }

        cr.move_to(polygon[0].0, polygon[0].1);
        for point in polygon.iter().skip(1) {
            cr.line_to(point.0, point.1);
        }
        cr.close_path();

        cr.set_source_rgb(color.0, color.1, color.2);
        cr.fill_preserve().unwrap();

        cr.set_source_rgb(0.16, 0.13, 0.10);
        cr.set_line_width(0.8);
        cr.stroke().unwrap();
    }
}
