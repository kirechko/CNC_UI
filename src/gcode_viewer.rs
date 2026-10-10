use gtk4::prelude::*;
use gtk4::{DrawingArea, GestureDrag, EventControllerScroll};
use std::path::PathBuf;
use std::f64::consts::PI;
use std::rc::Rc;
use std::cell::RefCell;

#[derive(Debug, Clone, Copy, PartialEq)]
enum MoveType { Rapid, Linear, Arc }

#[derive(Debug, Clone)]
struct Segment {
    x_start: f64, y_start: f64, z_start: f64,
    x_end: f64,   y_end: f64,   z_end: f64,
    move_type: MoveType,
}

// Храним ориентацию камеры как матрицу 3x3 для корректного орбитального вращения
#[derive(Clone)]
struct CameraOrientation {
    // Базисные векторы камеры (строки матрицы поворота)
    right: [f64; 3], // X-axis of camera
    up:    [f64; 3], // Y-axis of camera  
    fwd:   [f64; 3], // Z-axis of camera (view direction)
}

impl CameraOrientation {
    fn new() -> Self {
        // Начальная изометрическая ориентация
        Self {
            right: [1.0, 0.0, 0.0],
            up:    [0.0, 1.0, 0.0],
            fwd:   [0.0, 0.0, 1.0],
        }
    }

    // Вращение вокруг произвольной оси на угол theta
    fn rotate(&mut self, axis: [f64; 3], theta: f64) {
        let c = theta.cos();
        let s = theta.sin();
        let t = 1.0 - c;
        let x = axis[0];
        let y = axis[1];
        let z = axis[2];

        // Матрица поворота Родрига
        let m = [
            [t*x*x + c,     t*x*y - s*z, t*x*z + s*y],
            [t*x*y + s*z,   t*y*y + c,   t*y*z - s*x],
            [t*x*z - s*y,   t*y*z + s*x, t*z*z + c  ],
        ];

        // Применяем матрицу к каждому базисному вектору
        for v in [&mut self.right, &mut self.up, &mut self.fwd].iter_mut() {
            let nx = m[0][0]*v[0] + m[0][1]*v[1] + m[0][2]*v[2];
            let ny = m[1][0]*v[0] + m[1][1]*v[1] + m[1][2]*v[2];
            let nz = m[2][0]*v[0] + m[2][1]*v[1] + m[2][2]*v[2];
            v[0] = nx; v[1] = ny; v[2] = nz;
        }
    }

    // Проекция 3D точки на экран с учетом текущей ориентации
    fn project(&self, x: f64, y: f64, z: f64, pan_x: f64, pan_y: f64, 
               scale: f64, w: f64, h: f64) -> (f64, f64) {
        // Трансформируем точку в систему координат камеры
        let tx = self.right[0]*x + self.right[1]*y + self.right[2]*z;
        let ty = self.up[0]*x    + self.up[1]*y    + self.up[2]*z;
        let tz = self.fwd[0]*x   + self.fwd[1]*y   + self.fwd[2]*z;

        // Ортографическая проекция: игнорируем глубину (tz), используем tx/ty
        // Инвертируем Y для экранных координат
        let sx = tx * scale + pan_x + w / 2.0;
        let sy = -ty * scale + pan_y + h / 2.0;
        (sx, sy)
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

        let camera = Camera {
            orientation: CameraOrientation::new(),
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        };

        let cam_rc = Rc::new(RefCell::new(camera));

        // --- ВРАЩЕНИЕ (ЛКМ) - Орбитальное вокруг центра координат ---
        let gesture_rot = GestureDrag::new();
        gesture_rot.set_button(gtk4::gdk::BUTTON_PRIMARY);
        let cam_r = cam_rc.clone();
        let area_r = area.clone();
        
        gesture_rot.connect_drag_update(move |gesture, _, _| {
            let (dx, dy) = gesture.offset().unwrap_or((0.0, 0.0));
            
            let mut c = cam_r.borrow_mut();
            const ROT_SENS: f64 = 0.005;
            let angle = (dx*dx + dy*dy).sqrt() * ROT_SENS;
            
            if angle > 0.0001 {
                // Ось вращения лежит в плоскости экрана и перпендикулярна движению мыши
                // dx -> вращение вокруг вертикальной оси экрана (Up)
                // dy -> вращение вокруг горизонтальной оси экрана (Right)
                
                // Нормализуем направление движения мыши
                let len = (dx*dx + dy*dy).sqrt();
                let mx = dx / len;
                let my = dy / len;
                
                // Ось вращения в мировых координатах:
                // Движение вправо (mx>0) -> вращаем вокруг Up вектора камеры
                // Движение вниз (my>0) -> вращаем вокруг Right вектора камеры
                // Но знак важен: движение вниз должно опускать взгляд -> вращение вокруг Right
                
                let axis = [
                    c.orientation.right[0] * my + c.orientation.up[0] * (-mx),
                    c.orientation.right[1] * my + c.orientation.up[1] * (-mx),
                    c.orientation.right[2] * my + c.orientation.up[2] * (-mx),
                ];
                
                c.orientation.rotate(axis, angle);
            }
            
            area_r.queue_draw();
        });

        // --- ПАНОРАМИРОВАНИЕ (ПКМ) ---
        let prev_pan = Rc::new(RefCell::new((0.0, 0.0)));
        let gesture_pan = GestureDrag::new();
        gesture_pan.set_button(gtk4::gdk::BUTTON_SECONDARY);
        let cam_p = cam_rc.clone();
        let area_p = area.clone();
        
        let pp_begin = prev_pan.clone();
        gesture_pan.connect_drag_begin(move |g, _, _| {
            let mut p = pp_begin.borrow_mut();
            *p = g.offset().unwrap_or((0.0, 0.0));
        });
        
        let pp_update = prev_pan.clone();
        gesture_pan.connect_drag_update(move |g, _, _| {
            let curr = g.offset().unwrap_or((0.0, 0.0));
            let mut prev = pp_update.borrow_mut();
            
            let dx = curr.0 - prev.0;
            let dy = curr.1 - prev.1;
            *prev = curr;
            
            let mut c = cam_p.borrow_mut();
            c.pan_x += dx;
            c.pan_y += dy;
            area_p.queue_draw();
        });

        // --- ЗУМ (Колесико) ---
        let scroll_ctrl = EventControllerScroll::new(
            gtk4::EventControllerScrollFlags::VERTICAL
        );
        let cam_s = cam_rc.clone();
        let area_s = area.clone();
        scroll_ctrl.connect_scroll(move |_, _dx, dy| {
            let mut c = cam_s.borrow_mut();
            c.zoom *= if dy > 0.0 { 0.95 } else { 1.05 };
            c.zoom = c.zoom.clamp(0.1, 10.0);
            area_s.queue_draw();
            gtk4::glib::Propagation::Stop
        });

        area.add_controller(gesture_rot);
        area.add_controller(gesture_pan);
        area.add_controller(scroll_ctrl);

        // Базовая отрисовка фона и сетки
        area.set_draw_func(move |_, cr, width, height| {
            let w = width as f64;
            let h = height as f64;
            
            cr.set_source_rgb(0.15, 0.15, 0.15);
            cr.paint().unwrap();
            
            cr.set_source_rgba(0.25, 0.25, 0.25, 0.5);
            cr.set_line_width(1.0);
            let step = 50.0;
            for i in -10..=10 {
                let v = i as f64 * step;
                cr.move_to(w/2.0 + v, h/2.0 - v*0.5);
                cr.line_to(w/2.0 + v + 500.0, h/2.0 - (v+500.0)*0.5);
                cr.stroke().unwrap();
                
                cr.move_to(w/2.0 - v*0.5, h/2.0 + v);
                cr.line_to(w/2.0 - (v+500.0)*0.5, h/2.0 + v + 500.0);
                cr.stroke().unwrap();
            }
        });

        Self { 
            widget: area, 
            segments: Vec::new(),
            bounds: (-50.0, -50.0, -50.0, 50.0, 50.0, 50.0),
            camera: cam_rc,
        }
    }

    pub fn load_file(&mut self, path: &PathBuf) {
        self.segments.clear();
        
        // Сброс камеры при каждой загрузке
        let mut cam = self.camera.borrow_mut();
        cam.orientation = CameraOrientation::new();
        cam.zoom = 1.0;
        cam.pan_x = 0.0;
        cam.pan_y = 0.0;
        drop(cam);
        
        if let Ok(content) = std::fs::read_to_string(path) {
            self.parse_gcode(&content);
            self.calculate_bounds();
            self.update_draw();
            println!("✅ Loaded {} segments", self.segments.len());
        }
    }

    fn parse_gcode(&mut self, content: &str) {
        let mut cur_x = 0.0; let mut cur_y = 0.0; let mut cur_z = 0.0;
        let mut last_i = 0.0; let mut last_j = 0.0;
        let mut last_move = MoveType::Rapid;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') { continue; }
            let code = if let Some(p) = line.find(';') { &line[..p] } else { line };
            
            let mut nx=None; let mut ny=None; let mut nz=None; 
            let mut ni=None; let mut nj=None; let mut mt=None;

            for token in code.split_whitespace() {
                if token.len() < 2 { continue; }
                if let Ok(val) = token[1..].parse::<f64>() {
                    match &token[..1] {
                        "G" => match val as i32 {
                            0 => mt = Some(MoveType::Rapid),
                            1 => mt = Some(MoveType::Linear),
                            2 | 3 => mt = Some(MoveType::Arc),
                            _ => {}
                        },
                        "X" => nx = Some(val), "Y" => ny = Some(val), "Z" => nz = Some(val),
                        "I" => ni = Some(val), "J" => nj = Some(val),
                        _ => {}
                    }
                }
            }

            if let Some(x) = nx { cur_x = x; }
            if let Some(y) = ny { cur_y = y; }
            if let Some(z) = nz { cur_z = z; }
            if let Some(i) = ni { last_i = i; }
            if let Some(j) = nj { last_j = j; }

            let move_t = mt.unwrap_or(last_move);
            last_move = move_t;

            if move_t == MoveType::Arc {
                let cx = cur_x + last_i;
                let cy = cur_y + last_j;
                let r = (last_i*last_i + last_j*last_j).sqrt();
                let start_a = (cur_y - cy).atan2(cur_x - cx);
                let end_a = (ny.unwrap_or(cur_y) - cy).atan2(nx.unwrap_or(cur_x) - cx);
                let mut sweep = end_a - start_a;
                if sweep.abs() < 0.01 { sweep += 2.0 * PI; } 
                
                let steps = (sweep.abs() / 0.2).max(4.0) as usize;
                for i in 1..=steps {
                    let t = i as f64 / steps as f64;
                    let a = start_a + sweep * t;
                    let sx = cx + r * a.cos();
                    let sy = cy + r * a.sin();
                    let sz = cur_z + (nz.unwrap_or(cur_z) - cur_z) * t;
                    
                    let prev = if i==1 { (cur_x, cur_y, cur_z) } 
                               else { let l=self.segments.last().unwrap(); (l.x_end, l.y_end, l.z_end) };
                    
                    self.segments.push(Segment {
                        x_start: prev.0, y_start: prev.1, z_start: prev.2,
                        x_end: sx, y_end: sy, z_end: sz, move_type: MoveType::Arc
                    });
                }
                cur_x = nx.unwrap_or(cur_x); cur_y = ny.unwrap_or(cur_y); cur_z = nz.unwrap_or(cur_z);
            } else if nx.is_some() || ny.is_some() || nz.is_some() {
                self.segments.push(Segment {
                    x_start: cur_x, y_start: cur_y, z_start: cur_z,
                    x_end: nx.unwrap_or(cur_x), y_end: ny.unwrap_or(cur_y), z_end: nz.unwrap_or(cur_z),
                    move_type: move_t
                });
            }
        }
    }

    fn calculate_bounds(&mut self) {
        if self.segments.is_empty() { return; }
        let mut mn=(f64::MAX,f64::MAX,f64::MAX); let mut mx=(f64::MIN,f64::MIN,f64::MIN);
        for s in &self.segments {
            mn.0=mn.0.min(s.x_start).min(s.x_end); mx.0=mx.0.max(s.x_start).max(s.x_end);
            mn.1=mn.1.min(s.y_start).min(s.y_end); mx.1=mx.1.max(s.y_start).max(s.y_end);
            mn.2=mn.2.min(s.z_start).min(s.z_end); mx.2=mx.2.max(s.z_start).max(s.z_end);
        }
        self.bounds = (mn.0, mn.1, mn.2, mx.0, mx.1, mx.2);
    }

    pub fn update_draw(&mut self) {
        let segs = self.segments.clone();
        let b = self.bounds;
        let cam_rc = self.camera.clone();
        
        self.widget.set_draw_func(move |_, cr, width, height| {
            let w = width as f64; 
            let h = height as f64;
            
            cr.set_source_rgb(0.15, 0.15, 0.15); 
            cr.paint().unwrap();
            
            cr.set_source_rgba(0.25, 0.25, 0.25, 0.5);
            cr.set_line_width(1.0);
            let step = 50.0;
            for i in -10..=10 {
                let v = i as f64 * step;
                cr.move_to(w/2.0 + v, h/2.0 - v*0.5);
                cr.line_to(w/2.0 + v + 500.0, h/2.0 - (v+500.0)*0.5);
                cr.stroke().unwrap();
                cr.move_to(w/2.0 - v*0.5, h/2.0 + v);
                cr.line_to(w/2.0 - (v+500.0)*0.5, h/2.0 + v + 500.0);
                cr.stroke().unwrap();
            }

            let cam = cam_rc.borrow();

            let range = ((b.3-b.0) + (b.4-b.1) + (b.5-b.2)) / 3.0;
            let base_scale = if range > 0.001 { (w.min(h) / range) * 0.8 } else { 1.0 };
            let sc = base_scale * cam.zoom;

            let proj = |x: f64, y: f64, z: f64| -> (f64, f64) {
                cam.orientation.project(x, y, z, cam.pan_x, cam.pan_y, sc, w, h)
            };

            for s in &segs {
                let (x1, y1) = proj(s.x_start, s.y_start, s.z_start);
                let (x2, y2) = proj(s.x_end, s.y_end, s.z_end);
                
                match s.move_type {
                    MoveType::Rapid => {
                        cr.set_source_rgba(0.3, 0.3, 0.8, 0.5);
                        cr.set_dash(&[4.0, 4.0], 0.0); cr.set_line_width(1.0);
                    },
                    MoveType::Linear => {
                        cr.set_source_rgb(0.9, 0.7, 0.2);
                        cr.set_dash(&[], 0.0); cr.set_line_width(1.5);
                    },
                    MoveType::Arc => {
                        cr.set_source_rgb(0.2, 0.8, 0.4);
                        cr.set_dash(&[], 0.0); cr.set_line_width(1.5);
                    }
                }
                cr.move_to(x1, y1); cr.line_to(x2, y2); cr.stroke().unwrap();
            }

            // Оси координат
            let (ox, oy) = proj(0.0, 0.0, 0.0);
            let len = 20.0 * cam.zoom;
            
            cr.set_line_width(2.0); cr.set_dash(&[],0.0);
            
            let (ex, ey) = proj(len, 0.0, 0.0);
            cr.set_source_rgb(1.0, 0.3, 0.3);
            cr.move_to(ox, oy); cr.line_to(ex, ey); cr.stroke().unwrap();
            
            let (ex, ey) = proj(0.0, len, 0.0);
            cr.set_source_rgb(0.3, 1.0, 0.3);
            cr.move_to(ox, oy); cr.line_to(ex, ey); cr.stroke().unwrap();
            
            let (ex, ey) = proj(0.0, 0.0, len);
            cr.set_source_rgb(0.3, 0.6, 1.0);
            cr.move_to(ox, oy); cr.line_to(ex, ey); cr.stroke().unwrap();
        });
    }
}
