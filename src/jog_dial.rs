 //! Renders the XY jog dial and translates clicks into incremental jog commands.
use crate::cnc_controller::CNCController;
use gtk4::prelude::*;
use gtk4::DrawingArea;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

// Геометрия
const R_CENTER: f64 = 0.15;
const R_MAX: f64 = 1.0;
const RING_WIDTH: f64 = (R_MAX - R_CENTER) / 4.0;
const BOUND_1: f64 = R_CENTER + RING_WIDTH;
const BOUND_2: f64 = R_CENTER + 2.0 * RING_WIDTH;
const BOUND_3: f64 = R_CENTER + 3.0 * RING_WIDTH;

// ЦВЕТА ПОЛЬЗОВАТЕЛЯ (пересчитаны из HEX в f64)
// Ось X (Красные/Бордовые): 8d3837, 6e3332, 532e2e, 3f2a2b
const COLOR_X_RING4: (f64, f64, f64) = (0.553, 0.220, 0.216); // Внешнее (самое яркое) #8d3837
const COLOR_X_RING3: (f64, f64, f64) = (0.431, 0.200, 0.196); // #6e3332
const COLOR_X_RING2: (f64, f64, f64) = (0.325, 0.180, 0.180); // #532e2e
const COLOR_X_RING1: (f64, f64, f64) = (0.247, 0.165, 0.169); // Внутреннее (самое темное) #3f2a2b

// Ось Y (Зеленые/Хаки): 3d766c, 375e56, 314945, 2b3937
const COLOR_Y_RING4: (f64, f64, f64) = (0.239, 0.463, 0.424); // Внешнее #3d766c
const COLOR_Y_RING3: (f64, f64, f64) = (0.216, 0.369, 0.337); // #375e56
const COLOR_Y_RING2: (f64, f64, f64) = (0.192, 0.286, 0.271); // #314945
const COLOR_Y_RING1: (f64, f64, f64) = (0.169, 0.224, 0.216); // Внутреннее #2b3937

const COLOR_CENTER: (f64, f64, f64) = (0.12, 0.12, 0.13);

#[derive(Clone, Copy, PartialEq)]
struct HoveredJog {
    sector: usize,
    step: f64,
}

fn jog_selection(x: f64, y: f64, width: f64, height: f64) -> Option<HoveredJog> {
    let dx = x - width / 2.0;
    let dy = y - height / 2.0;
    let distance = (dx * dx + dy * dy).sqrt() / (width.min(height) / 2.0);

    let step = if distance < R_CENTER {
        return None;
    } else if distance < BOUND_1 {
        0.1
    } else if distance < BOUND_2 {
        1.0
    } else if distance < BOUND_3 {
        10.0
    } else if distance <= 1.10 {
        100.0
    } else {
        return None;
    };

    let mut angle = dy.atan2(dx);
    if angle < 0.0 {
        angle += 2.0 * std::f64::consts::PI;
    }

    let sector = ((angle / (std::f64::consts::PI / 4.0) + 0.5) as usize) % 8;
    Some(HoveredJog { sector, step })
}

fn jog_direction(sector: usize) -> &'static str {
    match sector {
        0 => "+X",
        1 => "+X / -Y",
        2 => "-Y",
        3 => "-X / -Y",
        4 => "-X",
        5 => "-X / +Y",
        6 => "+Y",
        _ => "+X / +Y",
    }
}

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

        let hover: Rc<Cell<Option<HoveredJog>>> = Rc::new(Cell::new(None));
        let draw_hover = hover.clone();

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
            let draw_ring_segment = |r_inner_frac: f64,
                                     r_outer_frac: f64,
                                     start: f64,
                                     end: f64,
                                     color: (f64, f64, f64)| {
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

            // Восемь секторов в каждом кольце:
            // осевые — прежних цветов, диагональные — синие.
            let blue_rings = [
                (49.0 / 255.0, 73.0 / 255.0, 101.0 / 255.0), // внутреннее #314965
                (70.0 / 255.0, 102.0 / 255.0, 139.0 / 255.0), // #46668b
                (79.0 / 255.0, 116.0 / 255.0, 158.0 / 255.0), // #4f749e
                (90.0 / 255.0, 133.0 / 255.0, 182.0 / 255.0), // внешнее #5a85b6
            ];

            let rings = [
                (R_CENTER, BOUND_1, COLOR_X_RING1, COLOR_Y_RING1),
                (BOUND_1, BOUND_2, COLOR_X_RING2, COLOR_Y_RING2),
                (BOUND_2, BOUND_3, COLOR_X_RING3, COLOR_Y_RING3),
                (BOUND_3, R_MAX, COLOR_X_RING4, COLOR_Y_RING4),
            ];

            for (ring_index, (r_inner, r_outer, color_x, color_y)) in rings.into_iter().enumerate()
            {
                for sector in 0..8 {
                    let center_angle = sector as f64 * std::f64::consts::FRAC_PI_4;
                    let start = center_angle - std::f64::consts::PI / 8.0;
                    let end = center_angle + std::f64::consts::PI / 8.0;

                    let color = match sector {
                        0 | 4 => color_x,            // +X / -X
                        2 | 6 => color_y,            // -Y / +Y
                        _ => blue_rings[ring_index], // диагональный jog XY
                    };

                    draw_ring_segment(r_inner, r_outer, start, end, color);
                }
            }

            if let Some(selection) = draw_hover.get() {
                let Some(ring_index) = (match selection.step {
                    0.1 => Some(0),
                    1.0 => Some(1),
                    10.0 => Some(2),
                    100.0 => Some(3),
                    _ => None,
                }) else {
                    return;
                };
                let (r_inner, r_outer, _, _) = rings[ring_index];
                let center_angle = selection.sector as f64 * std::f64::consts::FRAC_PI_4;
                let start = center_angle - std::f64::consts::PI / 8.0;
                let end = center_angle + std::f64::consts::PI / 8.0;
                let r_in = radius * r_inner;
                let r_out = radius * r_outer;

                cr.move_to(cx + r_in * start.cos(), cy + r_in * start.sin());
                cr.arc(cx, cy, r_out, start, end);
                cr.arc_negative(cx, cy, r_in, end, start);
                cr.close_path();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.28);
                cr.fill_preserve().unwrap();
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
                cr.set_line_width(2.0);
                cr.stroke().unwrap();
            }

            // Центральная шайба
            cr.arc(cx, cy, radius * R_CENTER, 0.0, 2.0 * std::f64::consts::PI);
            cr.set_source_rgb(COLOR_CENTER.0, COLOR_CENTER.1, COLOR_CENTER.2);
            cr.fill_preserve().unwrap();
            cr.set_source_rgb(0.05, 0.05, 0.06);
            cr.set_line_width(2.0);
            cr.stroke().unwrap();

            // Крестик в центре
            cr.move_to(cx - 6.0, cy);
            cr.line_to(cx + 6.0, cy);
            cr.move_to(cx, cy - 6.0);
            cr.line_to(cx, cy + 6.0);
            cr.set_source_rgb(0.35, 0.35, 0.36);
            cr.set_line_width(1.5);
            cr.stroke().unwrap();

            if let Some(selection) = draw_hover.get() {
                let step = if selection.step == 0.1 {
                    "0.1".to_string()
                } else {
                    format!("{:.0}", selection.step)
                };
                let text = format!("{} · {} mm", jog_direction(selection.sector), step);
                cr.select_font_face(
                    "Roboto",
                    gtk4::cairo::FontSlant::Normal,
                    gtk4::cairo::FontWeight::Bold,
                );
                cr.set_font_size(11.0);
                let extents = cr.text_extents(&text).unwrap();
                let text_x = cx - extents.width() / 2.0 - extents.x_bearing();
                let text_y = cy - radius * 0.42;
                cr.set_source_rgba(0.05, 0.05, 0.06, 0.88);
                cr.rectangle(
                    text_x - 5.0,
                    text_y + extents.y_bearing() - 3.0,
                    extents.width() + 10.0,
                    extents.height() + 6.0,
                );
                cr.fill().unwrap();
                cr.set_source_rgb(0.98, 0.98, 0.98);
                cr.move_to(text_x, text_y);
                cr.show_text(&text).unwrap();
            }

            // Подписи привязаны к радиусу, поэтому остаются по центру
            // соответствующих секторов внешнего кольца при любом размере виджета.
            let label_radius = radius * ((BOUND_3 + R_MAX) / 2.0);
            let label_font_size = (radius * 0.12).clamp(9.0, 21.0);

            cr.select_font_face(
                "Roboto",
                gtk4::cairo::FontSlant::Normal,
                gtk4::cairo::FontWeight::Normal,
            );
            cr.set_font_size(label_font_size);
            cr.set_source_rgb(0.95, 0.95, 0.95);

            for (text, angle) in [
                ("+X", 0.0),
                ("-X", std::f64::consts::PI),
                ("+Y", -std::f64::consts::FRAC_PI_2),
                ("-Y", std::f64::consts::FRAC_PI_2),
            ] {
                let (label_x, label_y) = if text == "-Y" {
                    (cx, cy + label_radius)
                } else {
                    (
                        cx + label_radius * angle.cos(),
                        cy + label_radius * angle.sin(),
                    )
                };

                let extents = cr.text_extents(text).unwrap();
                cr.move_to(
                    label_x - extents.x_bearing() - extents.width() / 2.0,
                    label_y - extents.y_bearing() - extents.height() / 2.0,
                );
                cr.show_text(text).unwrap();
            }
        });

        let motion_controller = gtk4::EventControllerMotion::new();
        let motion_hover = hover.clone();
        let motion_area = area.clone();
        motion_controller.connect_motion(move |_, x, y| {
            let allocation = motion_area.allocation();
            let selection = jog_selection(
                x,
                y,
                allocation.width() as f64,
                allocation.height() as f64,
            );
            if motion_hover.get() != selection {
                motion_hover.set(selection);
                motion_area.queue_draw();
            }
        });

        let leave_hover = hover.clone();
        let leave_area = area.clone();
        motion_controller.connect_leave(move |_| {
            if leave_hover.get().is_some() {
                leave_hover.set(None);
                leave_area.queue_draw();
            }
        });
        area.add_controller(motion_controller);

        // Логика кликов
        let click_gesture = gtk4::GestureClick::new();
        let ctrl_click = controller.clone();

        click_gesture.connect_released(move |gesture, _, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            let rect = gesture.widget().allocation();
            let Some(selection) = jog_selection(
                x,
                y,
                rect.width() as f64,
                rect.height() as f64,
            ) else {
                return;
            };
            let sector = selection.sector;
            // Угол 0 — вправо; экранная Y направлена вниз.
            let (axis, dir, diagonal) = match sector {
                0 => ('X', 1, None),
                1 => ('X', 1, Some(('Y', -1))), // юго-восток: +X, -Y
                2 => ('Y', -1, None),
                3 => ('X', -1, Some(('Y', -1))), // юго-запад: -X, -Y
                4 => ('X', -1, None),
                5 => ('X', -1, Some(('Y', 1))), // северо-запад: -X, +Y
                6 => ('Y', 1, None),
                _ => ('X', 1, Some(('Y', 1))), // северо-восток: +X, +Y
            };

            let step = selection.step;

            let c = ctrl_click.lock().unwrap();
            c.set_jog_step(step);

            if let Some((y_axis, y_dir)) = diagonal {
                debug_assert_eq!(axis, 'X');
                debug_assert_eq!(y_axis, 'Y');
                c.jog_xy_step(dir, y_dir, step);
            } else {
                c.jog_axis_step(axis, dir, step);
            }
        });

        area.add_controller(click_gesture);
        Self { widget: area }
    }
}
