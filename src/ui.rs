use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, FileChooserAction,
    FileFilter, Grid, Label, Orientation, Overlay, ResponseType, Scale,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::cnc_controller::{CNCController, SpindleDirection};
use crate::cnc_api::CoordinateSystem;
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

    // Автоматически загружаем test.nc из рабочего каталога, если файл существует.
    let startup_gcode = PathBuf::from("test.nc");
    if startup_gcode.is_file() {
        let mut viewer_guard = viewer.lock().unwrap();
        viewer_guard.load_file(&startup_gcode);
        viewer_guard.update_draw();
        drop(viewer_guard);

        if let Err(error) = controller.lock().unwrap().load_program(&startup_gcode) {
            eprintln!("[LinuxCncApi] load program: {}: {}", error.code, error.message);
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
    gtk4::glib::timeout_add_local(
        std::time::Duration::from_millis(50),
        move || {
            let status = telemetry_controller
                .lock()
                .unwrap()
                .get_status_snapshot();
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
        },
    );

    window.set_child(Some(&main_row));
    window.present();
}

fn create_left_panel(controller: Arc<Mutex<CNCController>>) -> (GtkBox, GtkBox) {
    let panel = GtkBox::new(Orientation::Vertical, 10);
    panel.add_css_class("box-panel");
    panel.set_vexpand(true);
    panel.set_hexpand(false);
    panel.set_size_request(340, -1);

    let pos_box = GtkBox::new(Orientation::Vertical, 5);
    let header = GtkBox::new(Orientation::Horizontal, 10);

    let title = Label::new(Some("POSITION"));
    title.add_css_class("heading");
    header.append(&title);

    let badge = Label::new(Some("● IDLE"));
    badge.add_css_class("status-badge");
    header.append(&badge);
    pos_box.append(&header);

    let coordinate_row = GtkBox::new(Orientation::Horizontal, 6);
    let coordinate_label = Label::new(Some("Coordinate system"));
    let coordinate_combo = gtk4::ComboBoxText::new();
    let coordinate_systems = CoordinateSystem::ALL;

    for system in coordinate_systems {
        coordinate_combo.append(Some(system.label()), system.label());
    }
    coordinate_combo.set_active_id(Some("G54"));

    let coordinate_controller = controller.clone();
    coordinate_combo.connect_changed(move |combo| {
        if let Some(id) = combo.active_id() {
            if let Some(system) = coordinate_systems
                .iter()
                .copied()
                .find(|system| system.label() == id.as_str())
            {
                coordinate_controller
                    .lock()
                    .unwrap()
                    .select_coordinate_system(system);
            }
        }
    });

    coordinate_row.append(&coordinate_label);
    coordinate_row.append(&coordinate_combo);
    pos_box.append(&coordinate_row);

    let coord_grid = Grid::new();
    coord_grid.set_row_spacing(5);
    coord_grid.set_column_spacing(10);

    let position_values = vec![
        Label::new(Some("0.000")),
        Label::new(Some("0.000")),
        Label::new(Some("0.000")),
    ];
    let machine_values = vec![
        Label::new(Some("0.000")),
        Label::new(Some("0.000")),
        Label::new(Some("0.000")),
    ];

    let work_header = Label::new(Some("WORK"));
    work_header.add_css_class("heading");
    let machine_header = Label::new(Some("MACHINE"));
    machine_header.add_css_class("heading");
    coord_grid.attach(&work_header, 1, 0, 1, 1);
    coord_grid.attach(&machine_header, 2, 0, 1, 1);

    for (i, (axis, color)) in [
        ("X", "#ff5252"),
        ("Y", "#69f0ae"),
        ("Z", crate::theme::ACCENT_BLUE),
    ]
    .iter()
    .enumerate()
    {
        let lbl = Label::new(None);
        lbl.set_markup(&format!(
            "<span foreground='{}' font_weight='bold'>{}</span>",
            color, axis
        ));

        let val = position_values[i].clone();
        val.add_css_class("monospace");
        val.set_halign(gtk4::Align::End);
        val.set_hexpand(true);

        let machine_val = machine_values[i].clone();
        machine_val.add_css_class("monospace");
        machine_val.set_halign(gtk4::Align::End);

        coord_grid.attach(&lbl, 0, i as i32 + 1, 1, 1);
        coord_grid.attach(&val, 1, i as i32 + 1, 1, 1);
        coord_grid.attach(&machine_val, 2, i as i32 + 1, 1, 1);
    }

    pos_box.append(&coord_grid);

    // Обновляем координаты из статуса эмулятора.
    let status_controller = controller.clone();
    let status_values = position_values.clone();
    let status_machine_values = machine_values.clone();
    let status_coordinate_combo = coordinate_combo.clone();
    let status_badge = badge.clone();
    let spindle_status = Label::new(Some("Stopped · 0 RPM"));
    spindle_status.add_css_class("monospace");
    pos_box.append(&spindle_status);

    let coolant_row = GtkBox::new(Orientation::Horizontal, 8);
    let mist_led = Label::new(Some("MIST"));
    let flood_led = Label::new(Some("FLOOD"));
    mist_led.add_css_class("coolant-led");
    flood_led.add_css_class("coolant-led");
    coolant_row.append(&mist_led);
    coolant_row.append(&flood_led);
    pos_box.append(&coolant_row);

    let event_log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let last_event = Rc::new(RefCell::new(None::<String>));
    let latest_event = Label::new(Some("No messages"));
    latest_event.add_css_class("event-log-latest");
    latest_event.set_halign(gtk4::Align::Start);
    latest_event.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    latest_event.set_max_width_chars(34);

    let mist_led_poll = mist_led.clone();
    let flood_led_poll = flood_led.clone();
    let event_log_poll = event_log.clone();
    let last_event_poll = last_event.clone();
    let latest_event_poll = latest_event.clone();

    let log_row = GtkBox::new(Orientation::Horizontal, 6);
    let log_button = Button::with_label("LOG");
    let log_label = latest_event.clone();
    let log_history = event_log.clone();
    let log_parent = panel.clone();
    log_button.connect_clicked(move |_| {
        let dialog = gtk4::Window::builder()
            .title("Controller event log")
            .default_width(600)
            .default_height(400)
            .transient_for(
                &log_parent.root()
                    .and_then(|root| root.downcast::<ApplicationWindow>().ok())
                    .expect("event log parent window"),
            )
            .modal(true)
            .build();

        let content = GtkBox::new(Orientation::Vertical, 6);
        content.set_margin_start(10);
        content.set_margin_end(10);
        content.set_margin_top(10);
        content.set_margin_bottom(10);

        let view = gtk4::TextView::new();
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        let buffer = view.buffer();
        let entries = log_history.borrow().join("\n");
        buffer.set_text(&entries);

        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_child(Some(&view));
        content.append(&scroll);

        let close = Button::with_label("Close");
        let dialog_clone = dialog.clone();
        close.connect_clicked(move |_| dialog_clone.close());
        content.append(&close);

        dialog.set_child(Some(&content));
        dialog.present();
        let _ = &log_label;
    });
    log_row.append(&log_button);
    log_row.append(&latest_event);

    gtk4::glib::timeout_add_local(
        std::time::Duration::from_millis(200),
        move || {
            let status = status_controller
                .lock()
                .unwrap()
                .get_status_snapshot();

            status_values[0].set_text(&format!("{:.3}", status.x_position));
            status_values[1].set_text(&format!("{:.3}", status.y_position));
            status_values[2].set_text(&format!("{:.3}", status.z_position));
            status_machine_values[0].set_text(&format!("{:.3}", status.machine_x));
            status_machine_values[1].set_text(&format!("{:.3}", status.machine_y));
            status_machine_values[2].set_text(&format!("{:.3}", status.machine_z));

            let active_id = status.active_coordinate_system.label();
            if status_coordinate_combo.active_id().as_deref() != Some(active_id) {
                status_coordinate_combo.set_active_id(Some(active_id));
            }

            let spindle_text = match status.spindle_direction {
                SpindleDirection::CW => "CW",
                SpindleDirection::CCW => "CCW",
                SpindleDirection::Off => "Stopped",
            };
            spindle_status.set_text(&format!(
                "{} · {:.0} RPM",
                spindle_text,
                status.spindle_actual_speed
            ));

            for (led, active) in [
                (&mist_led_poll, status.mist_coolant_on),
                (&flood_led_poll, status.flood_coolant_on),
            ] {
                if active {
                    led.add_css_class("coolant-led-on");
                } else {
                    led.remove_css_class("coolant-led-on");
                }
            }

            if let Some(message) = status.message.as_ref() {
                if last_event_poll.borrow().as_ref() != Some(message) {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let timestamp = now % 86_400;
                    let entry = format!(
                        "{:02}:{:02}:{:02}  {}",
                        timestamp / 3600,
                        (timestamp / 60) % 60,
                        timestamp % 60,
                        message
                    );
                    let mut history = event_log_poll.borrow_mut();
                    history.push(entry.clone());
                    if history.len() > 1000 {
                        history.remove(0);
                    }
                    latest_event_poll.set_text(&entry);
                    *last_event_poll.borrow_mut() = Some(message.clone());
                }
            }

            if status.state == "ALARM" {
                status_badge.set_markup("<span foreground='#ff4d4f'>● ALARM</span>");
            } else {
                status_badge.set_text(&format!("● {}", status.state));
            }
            if status.state == "ALARM" {
                status_badge.add_css_class("alarm-badge");
            } else {
                status_badge.remove_css_class("alarm-badge");
            }

            gtk4::glib::ControlFlow::Continue
        },
    );


    let sys_btns = GtkBox::new(Orientation::Horizontal, 5);
    let zero_btn = Button::with_label("Zero All");
    zero_btn.add_css_class("suggested-action");
    let zero_controller = controller.clone();
    let zero_system_combo = coordinate_combo.clone();
    zero_btn.connect_clicked(move |_| {
        let Some(id) = zero_system_combo.active_id() else { return; };
        if let Some(system) = CoordinateSystem::ALL
            .iter()
            .copied()
            .find(|system| system.label() == id.as_str())
        {
            zero_controller.lock().unwrap().set_work_zero(system);
        }
    });

    let home_btn = Button::with_label("Home All");
    let reset_alarm_btn = Button::with_label("Reset Alarm");
    reset_alarm_btn.add_css_class("suggested-action");
    let estop_btn = Button::with_label("E-Stop");
    estop_btn.add_css_class("destructive-action");

    let home_controller = controller.clone();
    home_btn.connect_clicked(move |_| {
        home_controller.lock().unwrap().home_all();
    });

    let reset_controller = controller.clone();
    reset_alarm_btn.connect_clicked(move |_| {
        reset_controller.lock().unwrap().reset_alarm();
    });

    let estop_controller = controller.clone();
    estop_btn.connect_clicked(move |_| {
        estop_controller.lock().unwrap().emergency_stop();
    });

    sys_btns.append(&zero_btn);
    sys_btns.append(&home_btn);
    sys_btns.append(&reset_alarm_btn);
    sys_btns.append(&estop_btn);
    pos_box.append(&sys_btns);

    panel.append(&pos_box);
    panel.append(&gtk4::Separator::new(Orientation::Horizontal));

    let jog_title = Label::new(Some("JOG"));
    jog_title.add_css_class("heading");
    jog_title.set_halign(gtk4::Align::Center);
    panel.append(&jog_title);

    let speed_row = GtkBox::new(Orientation::Horizontal, 8);
    speed_row.set_halign(gtk4::Align::Center);

    let xy_speed_label = Label::new(Some("XY mm/min"));
    let xy_speed = gtk4::SpinButton::with_range(1.0, 10000.0, 50.0);
    xy_speed.set_value(1000.0);
    xy_speed.set_width_chars(5);
    let xy_speed_controller = controller.clone();
    xy_speed.connect_value_changed(move |spin| {
        xy_speed_controller
            .lock()
            .unwrap()
            .set_xy_jog_speed(spin.value());
    });

    let z_speed_label = Label::new(Some("Z mm/min"));
    let z_speed = gtk4::SpinButton::with_range(1.0, 10000.0, 10.0);
    z_speed.set_value(300.0);
    z_speed.set_width_chars(5);
    let z_speed_controller = controller.clone();
    z_speed.connect_value_changed(move |spin| {
        z_speed_controller
            .lock()
            .unwrap()
            .set_z_jog_speed(spin.value());
    });

    speed_row.append(&xy_speed_label);
    speed_row.append(&xy_speed);
    speed_row.append(&z_speed_label);
    speed_row.append(&z_speed);
    panel.append(&speed_row);

    let jog_container = GtkBox::new(Orientation::Horizontal, 10);
    jog_container.set_halign(gtk4::Align::Center);
    jog_container.set_valign(gtk4::Align::Center);
    jog_container.set_vexpand(true);

    let dial = JogDial::new(controller.clone());
    let dial_overlay = Overlay::new();
    dial_overlay.set_size_request(240, 240);
    dial_overlay.set_halign(gtk4::Align::Center);
    dial_overlay.set_valign(gtk4::Align::Center);
    dial_overlay.set_child(Some(&dial.widget));

    jog_container.append(&dial_overlay);

    let z_col = GtkBox::new(Orientation::Vertical, 0);
    z_col.set_homogeneous(false);

    let z_indicator = |text: &str| {
        let btn = Button::new();
        btn.add_css_class("btn-z");
        btn.set_can_target(false);

        let label = Label::new(None);
        label.set_markup(&format!(
            "<span foreground='{}'>{}</span>",
            crate::theme::ACCENT_BLUE,
            text
        ));
        btn.set_child(Some(&label));
        btn
    };

    let z_step_button = |text: &str, direction: i8| {
        let btn = Button::with_label(text);
        btn.add_css_class("btn-z");

        let step: f64 = text.parse().expect("Z jog step must be numeric");
        let c = controller.clone();
        btn.connect_clicked(move |_| {
            c.lock().unwrap().jog_axis_step('Z', direction, step);
        });
        btn
    };

    z_col.append(&z_indicator("+Z"));
    z_col.append(&z_step_button("10", 1));
    z_col.append(&z_step_button("1", 1));
    z_col.append(&z_step_button("0.1", 1));

    // Компактный декоративный разделитель без минимальной высоты GTK-кнопки.
    let z_gap = gtk4::DrawingArea::new();
    z_gap.set_content_height(8);
    z_gap.set_size_request(-1, 8);
    z_gap.set_can_target(false);
    z_gap.set_draw_func(|_, cr, width, height| {
        let w = width as f64;
        let h = height as f64;
        cr.set_source_rgb(0.24, 0.24, 0.24);
        cr.rectangle(1.0, 1.0, (w - 2.0).max(0.0), (h - 2.0).max(0.0));
        cr.fill_preserve().unwrap();
        cr.set_source_rgb(0.34, 0.34, 0.34);
        cr.set_line_width(1.0);
        cr.stroke().unwrap();
    });
    z_col.append(&z_gap);

    z_col.append(&z_step_button("0.1", -1));
    z_col.append(&z_step_button("1", -1));
    z_col.append(&z_step_button("10", -1));
    z_col.append(&z_indicator("-Z"));

    jog_container.append(&z_col);
    panel.append(&jog_container);

    panel.append(&gtk4::Separator::new(Orientation::Horizontal));

    let spin_box = GtkBox::new(Orientation::Vertical, 5);
    let spin_lbl = Label::new(Some("SPINDLE"));
    spin_lbl.add_css_class("heading");
    spin_lbl.set_halign(gtk4::Align::Center);
    spin_box.append(&spin_lbl);

    let scale = Scale::with_range(Orientation::Horizontal, 0.0, 12000.0, 100.0);
    scale.set_value(0.0);
    let c_spin = controller.clone();
    scale.connect_value_changed(move |s| {
        c_spin.lock().unwrap().set_spindle_speed(s.value())
    });
    spin_box.append(&scale);

    let spin_btns = GtkBox::new(Orientation::Horizontal, 5);
    let cw = Button::with_label("CW");
    let ccw = Button::with_label("CCW");
    let stop = Button::with_label("STOP");
    stop.add_css_class("destructive-action");

    // Блокируем запуск направления в Alarm и перепроверяем статус при клике.
    let alarm_controller = controller.clone();
    let cw_poll = cw.clone();
    let ccw_poll = ccw.clone();
    gtk4::glib::timeout_add_local(
        std::time::Duration::from_millis(200),
        move || {
            let alarm = alarm_controller
                .lock()
                .unwrap()
                .get_status_snapshot()
                .state == "ALARM";
            cw_poll.set_sensitive(!alarm);
            ccw_poll.set_sensitive(!alarm);
            gtk4::glib::ControlFlow::Continue
        },
    );

    let c_cw = controller.clone();
    cw.connect_clicked(move |_| {
        let controller = c_cw.lock().unwrap();
        if controller.get_status_snapshot().state != "ALARM" {
            controller.set_spindle_direction(SpindleDirection::CW);
        }
    });

    let c_ccw = controller.clone();
    ccw.connect_clicked(move |_| {
        let controller = c_ccw.lock().unwrap();
        if controller.get_status_snapshot().state != "ALARM" {
            controller.set_spindle_direction(SpindleDirection::CCW);
        }
    });

    let c_stop = controller.clone();
    stop.connect_clicked(move |_| c_stop.lock().unwrap().stop_spindle());

    spin_btns.append(&cw);
    spin_btns.append(&ccw);
    spin_btns.append(&stop);
    spin_box.append(&spin_btns);
    panel.append(&spin_box);

    (panel, log_row)
}

fn create_center_panel(
    controller: Arc<Mutex<CNCController>>,
    window: ApplicationWindow,
    viewer: Arc<Mutex<GCodeViewer>>,
    initially_loaded_file: Option<PathBuf>,
    event_log_row: GtkBox,
) -> GtkBox {
    let panel = GtkBox::new(Orientation::Vertical, 0);
    panel.add_css_class("box-panel");
    panel.set_vexpand(true);
    panel.set_hexpand(true);

    let toolbar = GtkBox::new(Orientation::Horizontal, 10);
    toolbar.set_margin_bottom(10);

    let load_btn = Button::with_label(" Load File");
    load_btn.add_css_class("suggested-action");
    toolbar.append(&load_btn);

    let file_lbl = Label::new(Some("No file loaded"));
    if let Some(path) = initially_loaded_file.as_ref() {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Unknown");
        file_lbl.set_text(name);
    }
    file_lbl.add_css_class("heading");
    toolbar.append(&file_lbl);

    let program_stack = gtk4::Stack::new();
    program_stack.set_transition_type(gtk4::StackTransitionType::None);
    program_stack.set_hexpand(false);

    let start_btn = Button::with_label("▶");
    start_btn.add_css_class("suggested-action");
    start_btn.set_size_request(58, 29);
    start_btn.set_hexpand(false);
    program_stack.add_named(&start_btn, Some("stopped"));

    let program_controls = GtkBox::new(Orientation::Horizontal, 0);
    program_controls.set_spacing(0);
    let pause_resume_btn = Button::new();
    pause_resume_btn.set_size_request(29, 29);
    pause_resume_btn.set_hexpand(false);
    pause_resume_btn.set_tooltip_text(Some("Pause / Resume"));
    pause_resume_btn.add_css_class("suggested-action");

    let pause_icon_area = gtk4::DrawingArea::new();
    pause_icon_area.set_content_width(24);
    pause_icon_area.set_content_height(22);
    pause_icon_area.set_size_request(24, 22);
    let show_play_icon = std::rc::Rc::new(std::cell::Cell::new(false));
    let play_state = show_play_icon.clone();
    pause_icon_area.set_draw_func(move |_, cr, width, height| {
        let w = width as f64;
        let h = height as f64;
        cr.set_source_rgb(1.0, 1.0, 1.0);

        if play_state.get() {
            // Компактный треугольник Play.
            cr.move_to(w * 0.374, h * 0.276);
            cr.line_to(w * 0.696, h * 0.50);
            cr.line_to(w * 0.374, h * 0.724);
            cr.close_path();
            cr.fill().unwrap();
        } else {
            // Две толстые, разнесённые полосы Pause.
            let bar_w = 3.5;
            let bar_h = 11.2;
            let gap = 3.5;
            let x0 = (w - (bar_w * 2.0 + gap)) / 2.0;
            let y0 = (h - bar_h) / 2.0;
            cr.rectangle(x0, y0, bar_w, bar_h);
            cr.rectangle(x0 + bar_w + gap, y0, bar_w, bar_h);
            cr.fill().unwrap();
        }
    });
    pause_resume_btn.set_child(Some(&pause_icon_area));

    let stop_program_btn = Button::new();
    stop_program_btn.set_size_request(29, 29);
    stop_program_btn.set_hexpand(false);
    stop_program_btn.set_tooltip_text(Some("Stop, stop spindle, retract Z"));
    stop_program_btn.add_css_class("destructive-action");

    let stop_icon_area = gtk4::DrawingArea::new();
    stop_icon_area.set_content_width(24);
    stop_icon_area.set_content_height(22);
    stop_icon_area.set_size_request(24, 22);
    stop_icon_area.set_draw_func(|_, cr, width, height| {
        let side = 8.4;
        let x = (width as f64 - side) / 2.0;
        let y = (height as f64 - side) / 2.0;
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(x, y, side, side);
        cr.fill().unwrap();
    });
    stop_program_btn.set_child(Some(&stop_icon_area));

    program_controls.append(&pause_resume_btn);
    program_controls.append(&stop_program_btn);
    program_stack.add_named(&program_controls, Some("running"));
    program_stack.set_visible_child_name("stopped");
    toolbar.append(&program_stack);

    let start_controller = controller.clone();
    let start_viewer = viewer.clone();
    start_btn.connect_clicked(move |_| {
        // Большая Start начинает программу заново, поэтому сбрасываем старую подсветку.
        start_viewer.lock().unwrap().reset_path_progress();

        let controller = start_controller.lock().unwrap();
        if let Err(error) = controller.start_program() {
            eprintln!("[LinuxCncApi] start: {}: {}", error.code, error.message);
        }
    });

    let pause_controller = controller.clone();
    let pause_icon_area_click = pause_icon_area.clone();
    let click_play_state = show_play_icon.clone();
    pause_resume_btn.connect_clicked(move |_| {
        let controller = pause_controller.lock().unwrap();
        let snapshot = controller.get_status_snapshot();

        let result = if snapshot.state == "PAUSED" {
            controller.program_resume()
        } else if snapshot.state == "READING" {
            controller.program_pause()
        } else {
            return;
        };

        if let Err(error) = result {
            eprintln!("[LinuxCncApi] pause/resume: {}: {}", error.code, error.message);
        }

        let updated = controller.get_status_snapshot();
        click_play_state.set(updated.state == "PAUSED");
        pause_icon_area_click.queue_draw();
    });

    let stop_controller = controller.clone();
    stop_program_btn.connect_clicked(move |_| {
        let controller = stop_controller.lock().unwrap();
        if let Err(error) = controller.program_abort() {
            eprintln!("[LinuxCncApi] stop: {}: {}", error.code, error.message);
        }
    });

    // Выбор большой Start или пары кнопок следует за состоянием эмулятора.
    let state_controller = controller.clone();
    let state_stack = program_stack.clone();
    let state_pause_icon_area = pause_icon_area.clone();
    gtk4::glib::timeout_add_local(
        std::time::Duration::from_millis(200),
        move || {
            let status = state_controller
                .lock()
                .unwrap()
                .get_status_snapshot();

            if status.state == "READING" || status.state == "PAUSED" {
                state_stack.set_visible_child_name("running");
                show_play_icon.set(status.state == "PAUSED");
                state_pause_icon_area.queue_draw();
            } else {
                state_stack.set_visible_child_name("stopped");
            }

            gtk4::glib::ControlFlow::Continue
        },
    );

    let mode_3d = gtk4::ToggleButton::with_label("3D");
    let mode_ortho = gtk4::ToggleButton::with_label("Ortho");
    mode_ortho.set_group(Some(&mode_3d));
    mode_3d.set_active(true);

    let projection_viewer = viewer.clone();
    mode_3d.connect_toggled(move |button| {
        if button.is_active() {
            projection_viewer.lock().unwrap().set_perspective(true);
        }
    });

    let projection_viewer = viewer.clone();
    mode_ortho.connect_toggled(move |button| {
        if button.is_active() {
            projection_viewer.lock().unwrap().set_perspective(false);
        }
    });

    toolbar.append(&mode_3d);
    toolbar.append(&mode_ortho);

    for text in ["Orbit", "Rapid", "Tool"] {
        toolbar.append(&Button::with_label(text));
    }

    let center_button = Button::with_label("B");
    center_button.set_tooltip_text(Some("Center work origin"));
    let center_viewer = viewer.clone();
    center_button.connect_clicked(move |_| {
        center_viewer.lock().unwrap().center_work_origin();
    });
    toolbar.append(&center_button);
    panel.append(&toolbar);

    let file_label = file_lbl.clone();
    let viewer_for_dialog = viewer.clone();
    let controller_for_dialog = controller.clone();

    load_btn.connect_clicked(move |_| {
        let dialog = gtk4::FileChooserDialog::new(
            Some("Open G-Code File"),
            Some(&window),
            FileChooserAction::Open,
            &[
                ("Cancel", ResponseType::Cancel),
                ("Open", ResponseType::Accept),
            ],
        );

        let filter = FileFilter::new();
        filter.add_pattern("*.nc");
        filter.add_pattern("*.gcode");
        filter.set_name(Some("G-Code Files (*.nc, *.gcode)"));
        dialog.add_filter(&filter);

        let label = file_label.clone();
        let viewer = viewer_for_dialog.clone();
        let controller = controller_for_dialog.clone();

        dialog.connect_response(move |dialog, response| {
            if response == ResponseType::Accept {
                if let Some(path) = dialog.file().and_then(|file| file.path()) {
                    let filename = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Unknown");
                    label.set_text(filename);

                    {
                        let mut viewer = viewer.lock().unwrap();
                        viewer.load_file(&path);
                        viewer.update_draw();
                    }
                    if let Err(error) = controller.lock().unwrap().load_program(&path) {
                        eprintln!("[LinuxCncApi] load program: {}: {}", error.code, error.message);
                    }
                }
            }
            dialog.destroy();
        });

        dialog.present();
    });

    {
        let viewer = viewer.lock().unwrap();

        let scene_overlay = Overlay::new();
        scene_overlay.set_hexpand(true);
        scene_overlay.set_vexpand(true);
        scene_overlay.set_child(Some(&viewer.widget));

        let cube = viewer.orientation_widget.clone();
        cube.set_halign(gtk4::Align::End);
        cube.set_valign(gtk4::Align::Start);
        cube.set_margin_top(10);
        cube.set_margin_end(10);
        scene_overlay.add_overlay(&cube);

        panel.append(&scene_overlay);
    }

    event_log_row.set_hexpand(true);
    panel.append(&event_log_row);

    panel
}

fn create_right_panel(viewer: Arc<Mutex<GCodeViewer>>) -> GtkBox {
    let panel = GtkBox::new(Orientation::Vertical, 10);
    panel.add_css_class("box-panel");
    panel.set_vexpand(true);
    panel.set_hexpand(false);
    panel.set_size_request(250, -1);

    let tabs = GtkBox::new(Orientation::Horizontal, 10);
    tabs.append(&Button::with_label("FILES"));
    tabs.append(&Button::with_label("MACROS"));
    panel.append(&tabs);

    let list = GtkBox::new(Orientation::Vertical, 5);
    let files = ["test.nc", "Ray-Gear.nc", "Pocket-Test.nc", "Engrave-Logo.gcode"];

    for item in files {
        let label = Label::new(Some(item));
        label.set_halign(gtk4::Align::Start);

        let gesture = gtk4::GestureClick::new();
        let path_string = item.to_string();
        let viewer = viewer.clone();

        gesture.connect_released(move |_, _, _, _| {
            let path = PathBuf::from(&path_string);
            if path.exists() {
                let mut viewer = viewer.lock().unwrap();
                viewer.load_file(&path);
                viewer.update_draw();
                println!("Quick loaded: {}", path_string);
            } else {
                println!("File not found: {}", path_string);
            }
        });

        label.add_controller(gesture);
        list.append(&label);
    }

    panel.append(&list);
    panel.append(&gtk4::Separator::new(Orientation::Horizontal));

    let terminal_title = Label::new(Some("TERMINAL"));
    terminal_title.add_css_class("heading");
    panel.append(&terminal_title);

    let terminal = gtk4::TextView::new();
    terminal.set_vexpand(true);
    terminal.set_editable(false);
    panel.append(&terminal);

    panel
}
