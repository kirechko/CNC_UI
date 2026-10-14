mod center_panel;
mod left_panel;
mod right_panel;
use self::center_panel::create_center_panel;
use self::left_panel::create_left_panel;
use self::right_panel::create_right_panel;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, FileChooserAction, FileFilter, Grid,
    Label, Orientation, Overlay, ResponseType, Scale,
};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::cnc_api::CoordinateSystem;
use crate::cnc_controller::{CNCController, SpindleDirection};
use crate::gcode_viewer::GCodeViewer;
use crate::jog_dial::JogDial;

pub fn build_ui(app: &Application, controller: Arc<Mutex<CNCController>>) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("FluidNC Rust UI")
        .default_width(1200)
        .default_height(800)
        .build();

    let main_row = GtkBox::new(Orientation::Horizontal, 5);
    main_row.set_margin_start(5);
    main_row.set_margin_end(5);
    main_row.set_margin_top(5);
    main_row.set_margin_bottom(5);

    let (left_panel, event_log_row) = create_left_panel(controller.clone());
    main_row.append(&left_panel);

    let viewer = Arc::new(Mutex::new(GCodeViewer::new()));

    // Автоматически загружаем test.gcode из рабочего каталога, если файл существует.
    let startup_gcode = PathBuf::from("test.gcode");
    if startup_gcode.is_file() {
        let mut viewer_guard = viewer.lock().unwrap();
        viewer_guard.load_file(&startup_gcode);
        viewer_guard.update_draw();
        drop(viewer_guard);

        if let Err(error) = controller.lock().unwrap().load_program(&startup_gcode) {
            eprintln!(
                "[LinuxCncApi] load program: {}: {}",
                error.code, error.message
            );
        }
    }

    let center_panel = create_center_panel(
        controller.clone(),
        window.clone(),
        viewer.clone(),
        startup_gcode.is_file().then(|| startup_gcode.clone()),
        event_log_row,
    );
    main_row.append(&center_panel);

    let right_panel = create_right_panel(viewer.clone());
    main_row.append(&right_panel);

    // Передаём телеметрию станка в 3D-вид. Фаза вращения анимируется viewer-ом.
    let telemetry_controller = controller.clone();
    let telemetry_viewer = viewer.clone();
    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
        let status = telemetry_controller.lock().unwrap().get_status_snapshot();
        let viewer = telemetry_viewer.lock().unwrap();
        viewer.set_tool_telemetry(
            status.x_position,
            status.y_position,
            status.z_position,
            status.spindle_actual_speed,
            status.spindle_max_speed,
            status.spindle_direction == SpindleDirection::CW,
        );
        gtk4::glib::ControlFlow::Continue
    });

    window.set_child(Some(&main_row));
    window.present();
}
