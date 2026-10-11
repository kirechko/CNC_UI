use gtk4::prelude::*;
use gtk4::DrawingArea;
use std::sync::{Arc, Mutex};
use crate::cnc_controller::CNCController;

// Геометрия
const R_CENTER: f64 = 0.15;
const R_MAX: f64 = 1.0;
const RING_WIDTH: f64 = (R_MAX - R_CENTER) / 4.0;
const BOUND_1: f64 = R_CENTER + RING_WIDTH;
const BOUND_2: f64 = R_CENTER + 2.0 * RING_WIDTH;
const BOUND_3: f64 = R_CENTER + 3.0 * RING_WIDTH;

// ЦВЕТА ПОЛЬЗОВАТЕЛЯ (пересчитаны из HEX в f64)
// Ось X (Красные/Бордовые): 8d3837, 6e3332, 532e2e, 3f2a2b
const COLOR_X_RING4: (f64,f64,f64) = (0.553, 0.220, 0.216); // Внешнее (самое яркое) #8d3837
const COLOR_X_RING3: (f64,f64,f64) = (0.431, 0.200, 0.196); // #6e3332
const COLOR_X_RING2: (f64,f64,f64) = (0.325, 0.180, 0.180); // #532e2e
const COLOR_X_RING1: (f64,f64,f64) = (0.247, 0.165, 0.169); // Внутреннее (самое темное) #3f2a2b

// Ось Y (Зеленые/Хаки): 3d766c, 375e56, 314945, 2b3937
const COLOR_Y_RING4: (f64,f64,f64) = (0.239, 0.463, 0.424); // Внешнее #3d766c
const COLOR_Y_RING3: (f64,f64,f64) = (0.216, 0.369, 0.337); // #375e56
const COLOR_Y_RING2: (f64,f64,f64) = (0.192, 0.286, 0.271); // #314945
const COLOR_Y_RING1: (f64,f64,f64) = (0.169, 0.224, 0.216); // Внутреннее #2b3937

const COLOR_CENTER: (f64, f64, f64) = (0.12, 0.12, 0.13);

pub struct JogDial {
    pub widget: DrawingArea,
}

impl JogDial {
    pub fn new(controller: Arc<Mutex<CNCController>>) -> Self {
        let area = DrawingArea::new();
        area.set_content_width(240);
        area.set_content_height(240);
        area.set_vexpand(false);
        area.set_hexpand(false);
        area.set_size_request(240, 240);

        area.set_draw_func(move |_, cr, width, height| {
            let w = width as f64;
            let h = height as f64;
            let cx = w / 2.0;
            let cy = h / 2.0;
            let radius = (w.min(h) / 2.0) * 0.92;

            // Фон
            cr.set_source_rgb(0.145, 0.145, 0.149);
            cr.paint().unwrap();

            // Функция рисования одного кольца (дуги) сплошным цветом
            let draw_ring_segment = |r_inner_frac: f64, r_outer_frac: f64, 
                                     start: f64, end: f64, color: (f64,f64,f64)| {
                let r_in = radius * r_inner_frac;
                let r_out = radius * r_outer_frac;
                
                cr.move_to(cx + r_in * start.cos(), cy + r_in * start.sin());
                cr.arc(cx, cy, r_out, start, end);
                cr.arc_negative(cx, cy, r_in, end, start);
                cr.close_path();
                
                cr.set_source_rgb(color.0, color.1, color.2);
                cr.fill_preserve().unwrap();
                
                // Тонкая темная обводка
                cr.set_source_rgb(0.05, 0.05, 0.06);
                cr.set_line_width(1.5);
                cr.stroke().unwrap();
            };

            // Углы секторов
            const ANG_XP: (f64, f64) = (-std::f64::consts::PI/4.0, std::f64::consts::PI/4.0);
            const ANG_YN: (f64, f64) = (std::f64::consts::PI/4.0, 3.0*std::f64::consts::PI/4.0);
            const ANG_XN: (f64, f64) = (3.0*std::f64::consts::PI/4.0, 5.0*std::f64::consts::PI/4.0);
            const ANG_YP: (f64, f64) = (5.0*std::f64::consts::PI/4.0, 7.0*std::f64::consts::PI/4.0);

            // === РИСУЕМ КОЛЬЦА ПОСЛОЙНО (от внутреннего к внешнему) ===
            
            // Кольцо 1 (Самое темное, шаг 0.1)
            draw_ring_segment(R_CENTER, BOUND_1, ANG_XP.0, ANG_XP.1, COLOR_X_RING1);
            draw_ring_segment(R_CENTER, BOUND_1, ANG_YN.0, ANG_YN.1, COLOR_Y_RING1);
            draw_ring_segment(R_CENTER, BOUND_1, ANG_XN.0, ANG_XN.1, COLOR_X_RING1);
            draw_ring_segment(R_CENTER, BOUND_1, ANG_YP.0, ANG_YP.1, COLOR_Y_RING1);

            // Кольцо 2 (Шаг 1.0)
            draw_ring_segment(BOUND_1, BOUND_2, ANG_XP.0, ANG_XP.1, COLOR_X_RING2);
            draw_ring_segment(BOUND_1, BOUND_2, ANG_YN.0, ANG_YN.1, COLOR_Y_RING2);
            draw_ring_segment(BOUND_1, BOUND_2, ANG_XN.0, ANG_XN.1, COLOR_X_RING2);
            draw_ring_segment(BOUND_1, BOUND_2, ANG_YP.0, ANG_YP.1, COLOR_Y_RING2);

            // Кольцо 3 (Шаг 10.0)
            draw_ring_segment(BOUND_2, BOUND_3, ANG_XP.0, ANG_XP.1, COLOR_X_RING3);
            draw_ring_segment(BOUND_2, BOUND_3, ANG_YN.0, ANG_YN.1, COLOR_Y_RING3);
            draw_ring_segment(BOUND_2, BOUND_3, ANG_XN.0, ANG_XN.1, COLOR_X_RING3);
            draw_ring_segment(BOUND_2, BOUND_3, ANG_YP.0, ANG_YP.1, COLOR_Y_RING3);

            // Кольцо 4 (Самое яркое, шаг 100.0)
            draw_ring_segment(BOUND_3, R_MAX, ANG_XP.0, ANG_XP.1, COLOR_X_RING4);
            draw_ring_segment(BOUND_3, R_MAX, ANG_YN.0, ANG_YN.1, COLOR_Y_RING4);
            draw_ring_segment(BOUND_3, R_MAX, ANG_XN.0, ANG_XN.1, COLOR_X_RING4);
            draw_ring_segment(BOUND_3, R_MAX, ANG_YP.0, ANG_YP.1, COLOR_Y_RING4);
            
            // Центральная шайба
            cr.arc(cx, cy, radius * R_CENTER, 0.0, 2.0 * std::f64::consts::PI);
            cr.set_source_rgb(COLOR_CENTER.0, COLOR_CENTER.1, COLOR_CENTER.2);
            cr.fill_preserve().unwrap();
            cr.set_source_rgb(0.05, 0.05, 0.06);
            cr.set_line_width(2.0);
            cr.stroke().unwrap();
            
            // Крестик в центре
            cr.move_to(cx - 6.0, cy); cr.line_to(cx + 6.0, cy);
            cr.move_to(cx, cy - 6.0); cr.line_to(cx, cy + 6.0);
            cr.set_source_rgb(0.35, 0.35, 0.36);
            cr.set_line_width(1.5);
            cr.stroke().unwrap();
        });

        // Логика кликов
        let click_gesture = gtk4::GestureClick::new();
        let ctrl_click = controller.clone();
        
        click_gesture.connect_released(move |gesture, _, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            let rect = gesture.widget().allocation();
            let w = rect.width() as f64;
            let h = rect.height() as f64;
            let cx = w / 2.0;
            let cy = h / 2.0;
            
            let dx = x - cx;
            let dy = y - cy;
            let dist_norm = (dx*dx + dy*dy).sqrt() / (w.min(h)/2.0);
            
            let mut angle = dy.atan2(dx); 
            if angle < 0.0 { angle += 2.0 * std::f64::consts::PI; }

            let (axis, dir) = if (angle >= 0.0 && angle < std::f64::consts::PI/4.0) || (angle >= 7.0*std::f64::consts::PI/4.0) {
                ('X', 1)
            } else if angle >= std::f64::consts::PI/4.0 && angle < 3.0*std::f64::consts::PI/4.0 {
                ('Y', -1)
            } else if angle >= 3.0*std::f64::consts::PI/4.0 && angle < 5.0*std::f64::consts::PI/4.0 {
                ('X', -1)
            } else {
                ('Y', 1)
            };

            let step = if dist_norm < R_CENTER { return; }
                       else if dist_norm < BOUND_1 { 0.1_f64 }
                       else if dist_norm < BOUND_2 { 1.0_f64 }
                       else if dist_norm < BOUND_3 { 10.0_f64 }
                       else if dist_norm <= 1.10 { 100.0_f64 }
                       else { return; };

            let c = ctrl_click.lock().unwrap();
            c.set_jog_step(step);
            c.jog_axis_step(axis, dir, step);
        });

        area.add_controller(click_gesture);
        Self { widget: area }
    }
}
