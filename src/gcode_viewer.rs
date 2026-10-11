use gtk4::prelude::*;
use gtk4::{DrawingArea, EventControllerScroll, GestureClick, GestureDrag};
use std::cell::RefCell;
use std::f64::consts::PI;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq)]
enum MoveType {
    Rapid,
    Linear,
    Arc,
}

#[derive(Debug, Clone)]
struct Segment {
    x_start: f64,
    y_start: f64,
    z_start: f64,
    x_end: f64,
    y_end: f64,
    z_end: f64,
    move_type: MoveType,
}

#[derive(Clone)]
struct CameraOrientation {
    right: [f64; 3],
    up: [f64; 3],
    fwd: [f64; 3],
}

impl CameraOrientation {
    fn new() -> Self {
        // Изометрический вид: камера смотрит на сцену с направления (1, -1, 1).
        let mut orientation = Self {
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fwd: [0.0, 0.0, 1.0],
        };
        orientation.rotate([1.0, 0.0, 0.0], 35.264_f64.to_radians());
        orientation.rotate([0.0, 0.0, 1.0], 45.0_f64.to_radians());
        orientation
    }

    fn rotate(&mut self, axis: [f64; 3], theta: f64) {
        let length = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
        if length < 1e-12 {
            return;
        }

        let x = axis[0] / length;
        let y = axis[1] / length;
        let z = axis[2] / length;
        let c = theta.cos();
        let s = theta.sin();
        let t = 1.0 - c;

        let m = [
            [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
            [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
            [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
        ];

        for v in [&mut self.right, &mut self.up, &mut self.fwd] {
            let old = *v;
            *v = [
                m[0][0] * old[0] + m[0][1] * old[1] + m[0][2] * old[2],
                m[1][0] * old[0] + m[1][1] * old[1] + m[1][2] * old[2],
                m[2][0] * old[0] + m[2][1] * old[1] + m[2][2] * old[2],
            ];
        }
    }

    fn project(
        &self,
        x: f64,
        y: f64,
        z: f64,
        pan_x: f64,
        pan_y: f64,
        scale: f64,
        width: f64,
        height: f64,
    ) -> (f64, f64) {
        let tx = self.right[0] * x + self.right[1] * y + self.right[2] * z;
        let ty = self.up[0] * x + self.up[1] * y + self.up[2] * z;
        (
            tx * scale + pan_x + width / 2.0,
            -ty * scale + pan_y + height / 2.0,
        )
    }
}

#[derive(Clone)]
struct Camera {
    orientation: CameraOrientation,
    zoom: f64,
    pan_x: f64,
    pan_y: f64,
}

pub struct GCodeViewer {
    pub widget: DrawingArea,
    pub orientation_widget: DrawingArea,
    segments: Vec<Segment>,
    bounds: (f64, f64, f64, f64, f64, f64),
    camera: Rc<RefCell<Camera>>,
}

impl GCodeViewer {
    pub fn new() -> Self {
        let area = DrawingArea::new();
        area.set_vexpand(true);
        area.set_hexpand(true);
        area.set_can_target(true);
        area.set_cursor_from_name(Some("crosshair"));

        let orientation_widget = DrawingArea::new();
        orientation_widget.set_content_width(180);
        orientation_widget.set_content_height(180);
        orientation_widget.set_size_request(180, 180);
        orientation_widget.set_halign(gtk4::Align::Center);
        orientation_widget.set_can_target(true);

        let camera = Rc::new(RefCell::new(Camera {
            orientation: CameraOrientation::new(),
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        }));

        let cube_camera = camera.clone();
        orientation_widget.set_draw_func(move |_, cr, width, height| {
            draw_orientation_cube(cr, width as f64, height as f64, &cube_camera.borrow());
        });

        let drag = GestureDrag::new();
        drag.set_button(gtk4::gdk::BUTTON_PRIMARY);

        let drag_camera = camera.clone();
        let scene_area = area.clone();
        let cube_area = orientation_widget.clone();
        let previous = Rc::new(RefCell::new((0.0, 0.0)));

        let previous_begin = previous.clone();
        drag.connect_drag_begin(move |_, _, _| {
            *previous_begin.borrow_mut() = (0.0, 0.0);
        });

        drag.connect_drag_update(move |gesture, _, _| {
            let current = gesture.offset().unwrap_or((0.0, 0.0));
            let mut previous = previous.borrow_mut();
            let dx = current.0 - previous.0;
            let dy = current.1 - previous.1;
            *previous = current;
            drop(previous);

            let mut cam = drag_camera.borrow_mut();
            const ROT_SENS: f64 = 0.005;
            let axis = [
                cam.orientation.right[0] * -dy - cam.orientation.up[0] * dx,
                cam.orientation.right[1] * -dy - cam.orientation.up[1] * dx,
                cam.orientation.right[2] * -dy - cam.orientation.up[2] * dx,
            ];
            let angle = (dx * dx + dy * dy).sqrt() * ROT_SENS;
            cam.orientation.rotate(axis, angle);
            drop(cam);

            scene_area.queue_draw();
            cube_area.queue_draw();
        });
        area.add_controller(drag);

        let pan = GestureDrag::new();
        pan.set_button(gtk4::gdk::BUTTON_SECONDARY);
        let pan_camera = camera.clone();
        let pan_area = area.clone();
        let pan_previous = Rc::new(RefCell::new((0.0, 0.0)));

        let p = pan_previous.clone();
        pan.connect_drag_begin(move |_, _, _| *p.borrow_mut() = (0.0, 0.0));

        pan.connect_drag_update(move |gesture, _, _| {
            let current = gesture.offset().unwrap_or((0.0, 0.0));
            let mut previous = pan_previous.borrow_mut();
            let dx = current.0 - previous.0;
            let dy = current.1 - previous.1;
            *previous = current;
            drop(previous);

            let mut cam = pan_camera.borrow_mut();
            cam.pan_x += dx;
            cam.pan_y += dy;
            pan_area.queue_draw();
        });
        area.add_controller(pan);

        let scroll = EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
        let scroll_camera = camera.clone();
        let scroll_area = area.clone();
        scroll.connect_scroll(move |_, _, dy| {
            let mut cam = scroll_camera.borrow_mut();
            cam.zoom *= if dy > 0.0 { 0.95 } else { 1.05 };
            cam.zoom = cam.zoom.clamp(0.1, 10.0);
            drop(cam);
            scroll_area.queue_draw();
            gtk4::glib::Propagation::Stop
        });
        area.add_controller(scroll);

        let click = GestureClick::new();
        let click_camera = camera.clone();
        let click_scene = area.clone();
        let click_cube = orientation_widget.clone();
        click.connect_released(move |gesture, _, x, y| {
            let width = gesture.widget().width() as f64;
            let height = gesture.widget().height() as f64;
            let cam = click_camera.borrow();
            if let Some(normal) = cube_face_at(x, y, width, height, &cam) {
                drop(cam);
                click_camera.borrow_mut().orientation = orientation_for_face(normal);
                click_scene.queue_draw();
                click_cube.queue_draw();
            }
        });
        orientation_widget.add_controller(click);

        let draw_camera = camera.clone();
        area.set_draw_func(move |_, cr, width, height| {
            draw_scene(cr, width as f64, height as f64, &draw_camera.borrow(), &[], (-50.0, -50.0, -50.0, 50.0, 50.0, 50.0));
        });

        Self {
            widget: area,
            orientation_widget,
            segments: Vec::new(),
            bounds: (-50.0, -50.0, -50.0, 50.0, 50.0, 50.0),
            camera,
        }
    }

    pub fn load_file(&mut self, path: &PathBuf) {
        self.segments.clear();

        {
            let mut cam = self.camera.borrow_mut();
            cam.orientation = CameraOrientation::new();
            cam.zoom = 1.0;
            cam.pan_x = 0.0;
            cam.pan_y = 0.0;
        }

        if let Ok(content) = std::fs::read_to_string(path) {
            self.parse_gcode(&content);
            self.calculate_bounds();
            self.update_draw();
            self.orientation_widget.queue_draw();
            println!("Loaded {} segments", self.segments.len());
        }
    }

fn parse_gcode(&mut self, content: &str) {
    let (mut x, mut y, mut z) = (0.0, 0.0, 0.0);
    let (mut last_i, mut last_j) = (0.0, 0.0);
    let mut last_move = MoveType::Rapid;

    for line in content.lines() {
        let line = line.trim();

        if line.is_empty() || line.starts_with(';') {
            continue;
        }

        let code = line.split(';').next().unwrap_or("");
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
                    2 | 3 => mt = Some(MoveType::Arc),
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

        if let Some(value) = ni {
            last_i = value;
        }
        if let Some(value) = nj {
            last_j = value;
        }

        let move_type = mt.unwrap_or(last_move);
        last_move = move_type;

        let end_x = nx.unwrap_or(start_x);
        let end_y = ny.unwrap_or(start_y);
        let end_z = nz.unwrap_or(start_z);

        if move_type == MoveType::Arc && (nx.is_some() || ny.is_some() || ni.is_some() || nj.is_some()) {
            let center_x = start_x + last_i;
            let center_y = start_y + last_j;
            let radius = (last_i * last_i + last_j * last_j).sqrt();

            let start_angle = (start_y - center_y).atan2(start_x - center_x);
            let end_angle = (end_y - center_y).atan2(end_x - center_x);

            let mut sweep = end_angle - start_angle;
            if sweep.abs() < 0.01 {
                sweep += 2.0 * PI;
            }

            let steps = (sweep.abs() / 0.2).max(4.0) as usize;
            let mut previous = (start_x, start_y, start_z);

            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                let angle = start_angle + sweep * t;

                let next = (
                    center_x + radius * angle.cos(),
                    center_y + radius * angle.sin(),
                    start_z + (end_z - start_z) * t,
                );

                self.segments.push(Segment {
                    x_start: previous.0,
                    y_start: previous.1,
                    z_start: previous.2,
                    x_end: next.0,
                    y_end: next.1,
                    z_end: next.2,
                    move_type: MoveType::Arc,
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

    fn calculate_bounds(&mut self) {
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

    pub fn update_draw(&mut self) {
        let segments = self.segments.clone();
        let bounds = self.bounds;
        let camera = self.camera.clone();

        self.widget.set_draw_func(move |_, cr, width, height| {
            draw_scene(
                cr,
                width as f64,
                height as f64,
                &camera.borrow(),
                &segments,
                bounds,
            );
        });
        self.widget.queue_draw();
    }
}

fn draw_scene(
    cr: &gtk4::cairo::Context,
    width: f64,
    height: f64,
    camera: &Camera,
    segments: &[Segment],
    bounds: (f64, f64, f64, f64, f64, f64),
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
            x, y, z, camera.pan_x, camera.pan_y, scale, width, height,
        )
    };

    for segment in segments {
        let (x1, y1) = project(segment.x_start, segment.y_start, segment.z_start);
        let (x2, y2) = project(segment.x_end, segment.y_end, segment.z_end);

        match segment.move_type {
            MoveType::Rapid => {
                cr.set_source_rgba(0.3, 0.3, 0.8, 0.5);
                cr.set_dash(&[4.0, 4.0], 0.0);
                cr.set_line_width(1.0);
            }
            MoveType::Linear => {
                cr.set_source_rgb(0.9, 0.7, 0.2);
                cr.set_dash(&[], 0.0);
                cr.set_line_width(1.5);
            }
            MoveType::Arc => {
                cr.set_source_rgb(0.2, 0.8, 0.4);
                cr.set_dash(&[], 0.0);
                cr.set_line_width(1.5);
            }
        }
        cr.move_to(x1, y1);
        cr.line_to(x2, y2);
        cr.stroke().unwrap();
    }

    let (ox, oy) = project(0.0, 0.0, 0.0);
    let axis_len = 20.0 * camera.zoom;
    cr.set_line_width(2.0);
    cr.set_dash(&[], 0.0);

    for (end, color) in [
        (project(axis_len, 0.0, 0.0), (1.0, 0.3, 0.3)),
        (project(0.0, axis_len, 0.0), (0.3, 1.0, 0.3)),
        (project(0.0, 0.0, axis_len), crate::theme::ACCENT_BLUE_RGB),
    ] {
        cr.set_source_rgb(color.0, color.1, color.2);
        cr.move_to(ox, oy);
        cr.line_to(end.0, end.1);
        cr.stroke().unwrap();
    }
}

const CUBE_VERTICES: [[f64; 3]; 8] = [
    [-1.0, -1.0, -1.0],
    [ 1.0, -1.0, -1.0],
    [ 1.0,  1.0, -1.0],
    [-1.0,  1.0, -1.0],
    [-1.0, -1.0,  1.0],
    [ 1.0, -1.0,  1.0],
    [ 1.0,  1.0,  1.0],
    [-1.0,  1.0,  1.0],
];

const CUBE_FACES: [([usize; 4], [f64; 3], &str, (f64, f64, f64)); 6] = [
    ([1, 2, 6, 5], [ 1.0,  0.0,  0.0], "Right",  (0.50, 0.52, 0.56)),
    ([0, 4, 7, 3], [-1.0,  0.0,  0.0], "Left",   (0.42, 0.44, 0.48)),
    ([3, 7, 6, 2], [ 0.0,  1.0,  0.0], "Top",    (0.66, 0.67, 0.70)),
    ([0, 1, 5, 4], [ 0.0, -1.0,  0.0], "Bottom", (0.36, 0.38, 0.42)),
    ([4, 5, 6, 7], [ 0.0,  0.0,  1.0], "Front",  (0.56, 0.58, 0.61)),
    ([0, 3, 2, 1], [ 0.0,  0.0, -1.0], "Back",   (0.46, 0.48, 0.51)),
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


fn rounded_polygon_path(
    cr: &gtk4::cairo::Context,
    points: &[(f64, f64)],
    radius: f64,
) {
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

        let len_previous =
            (to_previous.0 * to_previous.0 + to_previous.1 * to_previous.1).sqrt();
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

fn draw_orientation_cube(
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

        // Подписи рисуем только на гранях, достаточно обращённых к камере.
        if facing >= 0.5 {
            cr.select_font_face(
                "Sans",
                gtk4::cairo::FontSlant::Normal,
                gtk4::cairo::FontWeight::Bold,
            );
            cr.set_font_size(11.0);
            cr.set_source_rgb(0.1, 0.1, 0.12);
            let extents = cr.text_extents(label).unwrap();
            cr.move_to(cx - extents.width() / 2.0, cy + extents.height() / 2.0);
            cr.show_text(label).unwrap();
        }
    }
}

fn cube_face_at(
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
        if (yi > y) != (yj > y)
            && x < (xj - xi) * (y - yi) / (yj - yi) + xi
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn orientation_for_face(normal: [f64; 3]) -> CameraOrientation {
    let (right, up, fwd) = if normal[2] > 0.5 {
        // Front (+Z): Y направлена вправо, X — вниз на экране.
        ([0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0])
    } else if normal[2] < -0.5 {
        ([0.0, -1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, -1.0])
    } else if normal[0] > 0.5 {
        ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0])
    } else if normal[0] < -0.5 {
        ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0])
    } else if normal[1] > 0.5 {
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
    } else {
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0])
    };

    CameraOrientation { right, up, fwd }
}
