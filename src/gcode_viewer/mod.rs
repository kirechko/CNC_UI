 //! Builds the G-code viewer widget and coordinates rendering, camera, and telemetry.
mod camera;
mod controls;
mod orientation_cube;
mod parser;
mod render;
mod tool;
use self::render::draw_scene;
mod types;
use self::types::{Camera, CameraOrientation, Segment, ToolVisual};
use gtk4::prelude::*;
use gtk4::DrawingArea;
use std::cell::RefCell;
use std::f64::consts::PI;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

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
        orientation_widget.set_content_width(120);
        orientation_widget.set_content_height(120);
        orientation_widget.set_size_request(120, 120);
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

        controls::attach_controls(&area, &orientation_widget, camera.clone());

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
        gtk4::glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
            let mut t = animation_tool.borrow_mut();
            let now = Instant::now();
            let dt = now.duration_since(t.last_update).as_secs_f64().min(0.1);
            t.last_update = now;
            if t.max_rpm > 0.0 && t.rpm > 0.0 {
                let rev_per_sec = (t.rpm / t.max_rpm).clamp(0.0, 1.0) * 5.0;
                t.phase =
                    (t.phase + t.direction_sign * rev_per_sec * 2.0 * PI * dt).rem_euclid(2.0 * PI);
            }
            drop(t);
            animation_area.queue_draw();
            gtk4::glib::ControlFlow::Continue
        });

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

    /// Сбрасывает масштаб и панорамирование, чтобы вся траектория помещалась в окне.
    pub fn fit_to_trajectory(&self) {
        let mut camera = self.camera.borrow_mut();
        camera.zoom = 1.0;
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
                let distance_sq = (x - px).powi(2) + (y - py).powi(2) + (z - pz).powi(2);

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
