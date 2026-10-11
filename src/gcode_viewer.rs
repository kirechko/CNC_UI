use gtk4::prelude::*;
use gtk4::{DrawingArea, EventControllerScroll, GestureClick, GestureDrag};
use std::cell::{Cell, RefCell};
use std::f64::consts::PI;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
enum MoveType {
    Rapid,
    Linear,
    ArcClockwise,
    ArcCounterClockwise,
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
        perspective: bool,
    ) -> (f64, f64) {
        let tx = self.right[0] * x + self.right[1] * y + self.right[2] * z;
        let ty = self.up[0] * x + self.up[1] * y + self.up[2] * z;
        let tz = self.fwd[0] * x + self.fwd[1] * y + self.fwd[2] * z;

        // Положительный tz направлен к камере. Ограничиваем знаменатель,
        // чтобы точки около плоскости камеры не давали бесконечную проекцию.
        let perspective_factor = if perspective {
            let camera_distance = 500.0;
            camera_distance / (camera_distance - tz).max(camera_distance * 0.1)
        } else {
            1.0
        };

        (
            tx * scale * perspective_factor + pan_x + width / 2.0,
            -ty * scale * perspective_factor + pan_y + height / 2.0,
        )
    }
}

#[derive(Clone)]
struct ToolVisual {
    x: f64,
    y: f64,
    z: f64,
    rpm: f64,
    max_rpm: f64,
    direction_sign: f64,
    phase: f64,
    last_update: Instant,
}

impl ToolVisual {
    fn new() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            rpm: 0.0,
            max_rpm: 12000.0,
            direction_sign: 1.0,
            phase: 0.0,
            last_update: Instant::now(),
        }
    }
}

#[derive(Clone)]
struct Camera {
    orientation: CameraOrientation,
    zoom: f64,
    pan_x: f64,
    pan_y: f64,
    perspective: bool,
}

pub struct GCodeViewer {
    pub widget: DrawingArea,
    pub orientation_widget: DrawingArea,
    segments: Vec<Segment>,
    telemetry_progress: Rc<std::cell::Cell<f64>>,
    bounds: (f64, f64, f64, f64, f64, f64),
    camera: Rc<RefCell<Camera>>,
    tool: Rc<RefCell<ToolVisual>>,
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
            perspective: true,
        }));
        let tool = Rc::new(RefCell::new(ToolVisual::new()));
        let telemetry_progress = Rc::new(std::cell::Cell::new(0.0));

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

        // Запоминаем позицию указателя, чтобы колесо масштабировало
        // сцену относительно курсора, а не центра DrawingArea.
        let pointer_position = Rc::new(Cell::new((0.0_f64, 0.0_f64)));
        let pointer_motion = gtk4::EventControllerMotion::new();
        let pointer_for_motion = pointer_position.clone();
        pointer_motion.connect_motion(move |_, x, y| {
            pointer_for_motion.set((x, y));
        });
        area.add_controller(pointer_motion);

        let scroll = EventControllerScroll::new(gtk4::EventControllerScrollFlags::VERTICAL);
        let scroll_camera = camera.clone();
        let scroll_area = area.clone();
        let scroll_pointer = pointer_position.clone();

        scroll.connect_scroll(move |_, _, dy| {
            let (pointer_x, pointer_y) = scroll_pointer.get();
            let width = scroll_area.width() as f64;
            let height = scroll_area.height() as f64;

            let mut cam = scroll_camera.borrow_mut();
            let old_zoom = cam.zoom;
            let new_zoom = (old_zoom * if dy > 0.0 { 0.95 } else { 1.05 })
                .clamp(0.1, 10.0);
            let zoom_ratio = new_zoom / old_zoom;

            // Сохраняем экранную позицию точки сцены под курсором.
            let cursor_x_from_center = pointer_x - width / 2.0;
            let cursor_y_from_center = pointer_y - height / 2.0;
            cam.pan_x = cursor_x_from_center
                - (cursor_x_from_center - cam.pan_x) * zoom_ratio;
            cam.pan_y = cursor_y_from_center
                - (cursor_y_from_center - cam.pan_y) * zoom_ratio;
            cam.zoom = new_zoom;
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
        let draw_tool = tool.clone();
        let draw_progress = telemetry_progress.clone();
        area.set_draw_func(move |_, cr, width, height| {
            draw_scene(
                cr,
                width as f64,
                height as f64,
                &draw_camera.borrow(),
                &[],
                (-50.0, -50.0, -50.0, 50.0, 50.0, 50.0),
                &draw_tool.borrow(),
                draw_progress.get(),
            );
        });

        // Анимация инструмента: 30 кадров/с, максимум 5 оборотов в секунду.
        let animation_area = area.clone();
        let animation_tool = tool.clone();
        gtk4::glib::timeout_add_local(
            std::time::Duration::from_millis(33),
            move || {
                let mut t = animation_tool.borrow_mut();
                let now = Instant::now();
                let dt = now.duration_since(t.last_update).as_secs_f64().min(0.1);
                t.last_update = now;
                if t.max_rpm > 0.0 && t.rpm > 0.0 {
                    let rev_per_sec = (t.rpm / t.max_rpm).clamp(0.0, 1.0) * 5.0;
                    t.phase = (t.phase
                        + t.direction_sign * rev_per_sec * 2.0 * PI * dt)
                        .rem_euclid(2.0 * PI);
                }
                drop(t);
                animation_area.queue_draw();
                gtk4::glib::ControlFlow::Continue
            },
        );

        Self {
            widget: area,
            orientation_widget,
            segments: Vec::new(),
            telemetry_progress,
            bounds: (-50.0, -50.0, -50.0, 50.0, 50.0, 50.0),
            camera,
            tool,
        }
    }

    /// Передаёт в 3D-сцену текущую телеметрию инструмента.
    pub fn set_perspective(&self, enabled: bool) {
        self.camera.borrow_mut().perspective = enabled;
        self.widget.queue_draw();
    }

    pub fn center_work_origin(&self) {
        let mut camera = self.camera.borrow_mut();
        camera.pan_x = 0.0;
        camera.pan_y = 0.0;
        drop(camera);
        self.widget.queue_draw();
    }

    pub fn reset_path_progress(&self) {
        self.telemetry_progress.set(0.0);
        self.widget.queue_draw();
    }

    pub fn set_tool_telemetry(
        &self,
        x: f64,
        y: f64,
        z: f64,
        actual_rpm: f64,
        max_rpm: f64,
        clockwise: bool,
    ) {
        {
            let mut tool = self.tool.borrow_mut();
            tool.x = x;
            tool.y = y;
            tool.z = z;
            tool.rpm = actual_rpm.max(0.0);
            tool.max_rpm = max_rpm.max(1.0);
            tool.direction_sign = if clockwise { 1.0 } else { -1.0 };
        }

        // Проецируем телеметрическую позицию на сегменты только впереди
        // достигнутой точки. Прогресс монотонный, что важно для повторных проходов.
        let previous = self.telemetry_progress.get();
        let count = self.segments.len();
        if count > 0 {
            let mut best_progress = previous.clamp(0.0, count as f64);
            let mut best_distance_sq = f64::INFINITY;
            let start_index = previous.floor().max(0.0) as usize;

            // Ищем только поблизости от прежней позиции прогресса:
            // повторяющиеся контуры не должны вызывать скачок к концу траектории.
            let search_end = count.min(start_index.saturating_add(12));
            for index in start_index..search_end {
                let segment = &self.segments[index];
                let dx = segment.x_end - segment.x_start;
                let dy = segment.y_end - segment.y_start;
                let dz = segment.z_end - segment.z_start;
                let length_sq = dx * dx + dy * dy + dz * dz;

                let mut t = if length_sq > 1e-12 {
                    ((x - segment.x_start) * dx
                        + (y - segment.y_start) * dy
                        + (z - segment.z_start) * dz)
                        / length_sq
                } else {
                    1.0
                };

                // На текущем сегменте не допускаем отката до уже пройденной части.
                if index == start_index {
                    t = t.max(previous.fract());
                }
                t = t.clamp(0.0, 1.0);

                let px = segment.x_start + dx * t;
                let py = segment.y_start + dy * t;
                let pz = segment.z_start + dz * t;
                let distance_sq =
                    (x - px).powi(2) + (y - py).powi(2) + (z - pz).powi(2);

                if distance_sq < best_distance_sq {
                    best_distance_sq = distance_sq;
                    best_progress = index as f64 + t;
                }
            }

            if best_distance_sq.is_finite() {
                let next_progress = previous.max(best_progress);
                self.telemetry_progress.set(next_progress);

            }
        }

        self.widget.queue_draw();
    }

    pub fn load_file(&mut self, path: &PathBuf) {
        self.segments.clear();
        self.telemetry_progress.set(0.0);

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

        if let Some(v) = ni { last_i = v; }
        if let Some(v) = nj { last_j = v; }

        let move_type = mt.unwrap_or(last_move);
        last_move = move_type;

        let end_x = nx.unwrap_or(start_x);
        let end_y = ny.unwrap_or(start_y);
        let end_z = nz.unwrap_or(start_z);

        if matches!(move_type, MoveType::ArcClockwise | MoveType::ArcCounterClockwise)
            && (nx.is_some() || ny.is_some() || ni.is_some() || nj.is_some())
        {
            let cx = start_x + last_i;
            let cy = start_y + last_j;
            let radius = (last_i * last_i + last_j * last_j).sqrt();
            let start_angle = (start_y - cy).atan2(start_x - cx);
            let end_angle = (end_y - cy).atan2(end_x - cx);
            let same_xy = (end_x - start_x).abs() < 1e-8
                && (end_y - start_y).abs() < 1e-8;

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
        let tool = self.tool.clone();
        let progress = self.telemetry_progress.clone();

        self.widget.set_draw_func(move |_, cr, width, height| {
            draw_scene(
                cr,
                width as f64,
                height as f64,
                &camera.borrow(),
                &segments,
                bounds,
                &tool.borrow(),
                progress.get(),
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

fn draw_hex_pencil<F>(
    cr: &gtk4::cairo::Context,
    project: F,
    camera: &Camera,
    tool: &ToolVisual,
)
where
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

    let top_ring: Vec<[f64; 3]> = ring
        .iter()
        .map(|p| [p[0], p[1], body_top_z])
        .collect();

    let tip = [tool.x, tool.y, tip_z];

    // Каждая грань хранит среднюю глубину и экранный полигон.
    // Сначала рисуются дальние грани, затем ближние.
    let mut faces: Vec<(f64, Vec<(f64, f64)>, (f64, f64, f64))> = Vec::new();

    let camera_depth = |p: [f64; 3]| {
        camera.orientation.fwd[0] * p[0]
            + camera.orientation.fwd[1] * p[1]
            + camera.orientation.fwd[2] * p[2]
    };

    for i in 0..SIDES {
        let j = (i + 1) % SIDES;

        // Боковая грань корпуса.
        let body_points = [
            ring[i],
            ring[j],
            top_ring[j],
            top_ring[i],
        ];
        let body_depth = body_points.iter().map(|p| camera_depth(*p)).sum::<f64>() / 4.0;
        let body_poly = body_points.iter().map(|p| project(p[0], p[1], p[2])).collect();
        // Одна золотистая грань служит меткой вращения; красного цвета нет.
        let base_color = match i {
            0 => (0.92, 0.66, 0.25),
            1 => (0.78, 0.56, 0.24),
            2 => (0.58, 0.38, 0.16),
            3 => (0.86, 0.62, 0.25),
            4 => (0.68, 0.44, 0.18),
            _ => (0.48, 0.31, 0.13),
        };

        // На остановке корпус темнее; при вращении появляется движущийся блик.
        // Яркость переключается только по факту вращения, не зависит от RPM.
        let brightness: f64 = if tool.rpm > 0.01 { 1.0 } else { 0.35 };
        let body_color = (
            (base_color.0 * brightness).min(1.0),
            (base_color.1 * brightness).min(1.0),
            (base_color.2 * brightness).min(1.0),
        );
        faces.push((body_depth, body_poly, body_color));

        // Треугольная заточка использует те же вершины кольца — щели быть не может.
        let tip_points = [tip, ring[i], ring[j]];
        let tip_depth = tip_points.iter().map(|p| camera_depth(*p)).sum::<f64>() / 3.0;
        let tip_poly = tip_points.iter().map(|p| project(p[0], p[1], p[2])).collect();
        let tip_shade: f64 = if i % 2 == 0 { 0.78 } else { 0.55 };
        let tip_brightness = brightness;
        faces.push((
            tip_depth,
            tip_poly,
            (
                (0.78 * tip_shade * tip_brightness).min(1.0),
                (0.56 * tip_shade * tip_brightness).min(1.0),
                (0.24 * tip_shade * tip_brightness).min(1.0),
            ),
        ));
    }

    // Закрываем верх корпуса шестигранной крышкой.
    let top_depth = top_ring.iter().map(|p| camera_depth(*p)).sum::<f64>() / SIDES as f64;
    let top_poly = top_ring.iter().map(|p| project(p[0], p[1], p[2])).collect();
    faces.push((top_depth, top_poly, (0.86, 0.62, 0.25)));

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
                let font_size = (11.0 * available_width / initial.width().max(1.0))
                    .clamp(6.0, 11.0);
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
