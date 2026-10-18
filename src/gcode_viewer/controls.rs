 //! Connects mouse gestures and orientation-cube clicks to camera controls.
use super::camera::orientation_for_face;
use super::orientation_cube::{cube_face_at, draw_orientation_cube};
use super::types::Camera;
use gtk4::prelude::*;
use gtk4::{DrawingArea, EventControllerScroll, GestureClick, GestureDrag};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(super) fn attach_controls(
    area: &DrawingArea,
    orientation_widget: &DrawingArea,
    camera: Rc<RefCell<Camera>>,
) {
    let cube_camera = camera.clone();
    orientation_widget.set_draw_func(move |_, cr, width, height| {
        draw_orientation_cube(cr, width as f64, height as f64, &cube_camera.borrow());
    });


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
        let new_zoom = (old_zoom * if dy > 0.0 { 0.95 } else { 1.05 }).clamp(0.1, 10.0);
        let zoom_ratio = new_zoom / old_zoom;

        // Сохраняем экранную позицию точки сцены под курсором.
        let cursor_x_from_center = pointer_x - width / 2.0;
        let cursor_y_from_center = pointer_y - height / 2.0;
        cam.pan_x = cursor_x_from_center - (cursor_x_from_center - cam.pan_x) * zoom_ratio;
        cam.pan_y = cursor_y_from_center - (cursor_y_from_center - cam.pan_y) * zoom_ratio;
        cam.zoom = new_zoom;
        drop(cam);

        scroll_area.queue_draw();
        gtk4::glib::Propagation::Stop
    });
    area.add_controller(scroll);

}
