use super::types::{Camera, MoveType, Segment, ToolVisual};
use super::tool::draw_hex_pencil;

pub(super) fn draw_scene(
    cr: &gtk4::cairo::Context,
    width: f64,
    height: f64,
    camera: &Camera,
    segments: &[Segment],
    bounds: (f64, f64, f64, f64, f64, f64),
    tool: &ToolVisual,
    progress: f64,
) {
    cr.set_source_rgb(0.15, 0.15, 0.15);
    cr.paint().unwrap();

    cr.set_source_rgba(0.25, 0.25, 0.25, 0.5);
    cr.set_line_width(1.0);
    for i in -10..=10 {
        let v = i as f64 * 50.0;
        cr.move_to(width / 2.0 + v, height / 2.0 - v * 0.5);
        cr.line_to(width / 2.0 + v + 500.0, height / 2.0 - (v + 500.0) * 0.5);
        cr.stroke().unwrap();
        cr.move_to(width / 2.0 - v * 0.5, height / 2.0 + v);
        cr.line_to(width / 2.0 - (v + 500.0) * 0.5, height / 2.0 + v + 500.0);
        cr.stroke().unwrap();
    }

    let range = ((bounds.3 - bounds.0) + (bounds.4 - bounds.1) + (bounds.5 - bounds.2)) / 3.0;
    let scale = if range > 0.001 {
        (width.min(height) / range) * 0.8 * camera.zoom
    } else {
        camera.zoom
    };

    let project = |x, y, z| {
        camera.orientation.project(
            x, y, z, camera.pan_x, camera.pan_y, scale, width, height, camera.perspective,
        )
    };

    for (index, segment) in segments.iter().enumerate() {
        let (x1, y1) = project(segment.x_start, segment.y_start, segment.z_start);
        let (x2, y2) = project(segment.x_end, segment.y_end, segment.z_end);
        let fraction = (progress - index as f64).clamp(0.0, 1.0);

        let (base_color, line_width, dashed) = match segment.move_type {
            MoveType::Rapid => ((0.48, 0.68, 1.0), 1.0, true),
            MoveType::Linear => ((0.9, 0.7, 0.2), 1.5, false),
            MoveType::ArcClockwise | MoveType::ArcCounterClockwise => {
                ((0.2, 0.8, 0.4), 1.5, false)
            }
        };

        let faded_color = (
            base_color.0 * 0.32,
            base_color.1 * 0.32,
            base_color.2 * 0.32,
        );

        cr.set_line_width(line_width);
        if dashed {
            cr.set_dash(&[4.0, 4.0], 0.0);
        } else {
            cr.set_dash(&[], 0.0);
        }

        // Пройденную долю рисуем тёмным непрозрачным цветом,
        // а остаток сегмента — обычным цветом.
        if fraction > 0.0 {
            let faded_end = (
                x1 + (x2 - x1) * fraction,
                y1 + (y2 - y1) * fraction,
            );
            cr.set_source_rgb(faded_color.0, faded_color.1, faded_color.2);
            cr.move_to(x1, y1);
            cr.line_to(faded_end.0, faded_end.1);
            cr.stroke().unwrap();
        }

        if fraction < 1.0 {
            let active_start = (
                x1 + (x2 - x1) * fraction,
                y1 + (y2 - y1) * fraction,
            );
            cr.set_source_rgb(base_color.0, base_color.1, base_color.2);
            cr.move_to(active_start.0, active_start.1);
            cr.line_to(x2, y2);
            cr.stroke().unwrap();
        }
    }

    let (ox, oy) = project(0.0, 0.0, 0.0);
    let axis_len = 20.0 * camera.zoom;
    cr.set_line_width(2.0);
    cr.set_dash(&[], 0.0);

    for (end, color, label) in [
        (project(axis_len, 0.0, 0.0), (1.0, 0.3, 0.3), "X"),
        (project(0.0, axis_len, 0.0), (0.3, 1.0, 0.3), "Y"),
        (project(0.0, 0.0, axis_len), crate::theme::ACCENT_BLUE_RGB, "Z"),
    ] {
        draw_axis_arrow(cr, (ox, oy), end, color, label);
    }

    draw_hex_pencil(cr, project, camera, tool);
}

fn draw_axis_arrow(
    cr: &gtk4::cairo::Context,
    start: (f64, f64),
    end: (f64, f64),
    color: (f64, f64, f64),
    label: &str,
) {
    let dx = end.0 - start.0;
    let dy = end.1 - start.1;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1.0 {
        return;
    }

    let ux = dx / length;
    let uy = dy / length;
    let head_len: f64 = 9.0;
    let head_width: f64 = 4.5;
    let base_x = end.0 - ux * head_len;
    let base_y = end.1 - uy * head_len;

    cr.set_source_rgb(color.0, color.1, color.2);
    cr.set_line_width(2.0);
    cr.move_to(start.0, start.1);
    cr.line_to(end.0, end.1);
    cr.stroke().unwrap();

    cr.move_to(end.0, end.1);
    cr.line_to(base_x - uy * head_width, base_y + ux * head_width);
    cr.line_to(base_x + uy * head_width, base_y - ux * head_width);
    cr.close_path();
    cr.fill().unwrap();

    cr.select_font_face(
        "Sans",
        gtk4::cairo::FontSlant::Normal,
        gtk4::cairo::FontWeight::Bold,
    );
    cr.set_font_size(12.0);
    let extents = cr.text_extents(label).unwrap();
    cr.move_to(
        end.0 + ux * 7.0 - extents.width() / 2.0,
        end.1 + uy * 7.0 - extents.height() / 2.0,
    );
    cr.show_text(label).unwrap();
}
