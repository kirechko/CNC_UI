use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, FileChooserAction,
    FileFilter, Grid, Label, Orientation, Overlay, ResponseType, Scale,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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

    let left_panel = create_left_panel(controller.clone());
    main_row.append(&left_panel);

    let viewer = Arc::new(Mutex::new(GCodeViewer::new()));

    // Автоматически загружаем test.nc из рабочего каталога, если файл существует.
    let startup_gcode = PathBuf::from("test.nc");
    if startup_gcode.is_file() {
        let mut viewer_guard = viewer.lock().unwrap();
        viewer_guard.load_file(&startup_gcode);
        viewer_guard.update_draw();
    }

    let center_panel = create_center_panel(controller.clone(), window.clone(), viewer.clone());
    main_row.append(&center_panel);

    let right_panel = create_right_panel(viewer.clone());
    main_row.append(&right_panel);

    window.set_child(Some(&main_row));
    window.present();
}

fn create_left_panel(controller: Arc<Mutex<CNCController>>) -> GtkBox {
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

    let coord_grid = Grid::new();
    coord_grid.set_row_spacing(5);
    coord_grid.set_column_spacing(10);

    for (i, (axis, color)) in [("X", "#ff5252"), ("Y", "#69f0ae"), ("Z", crate::theme::ACCENT_BLUE)]
        .iter()
        .enumerate()
    {
        let lbl = Label::new(None);
        lbl.set_markup(&format!(
            "<span foreground='{}' font_weight='bold'>{}</span>",
            color, axis
        ));
        let val = Label::new(Some("0.000"));
        val.add_css_class("monospace");
        val.set_halign(gtk4::Align::End);
        val.set_hexpand(true);
        coord_grid.attach(&lbl, 0, i as i32, 1, 1);
        coord_grid.attach(&val, 1, i as i32, 1, 1);
    }

    pos_box.append(&coord_grid);

    let sys_btns = GtkBox::new(Orientation::Horizontal, 5);
    let zero_btn = Button::with_label("Zero All");
    zero_btn.add_css_class("suggested-action");
    let home_btn = Button::with_label("Home All");
    sys_btns.append(&zero_btn);
    sys_btns.append(&home_btn);
    pos_box.append(&sys_btns);

    panel.append(&pos_box);
    panel.append(&gtk4::Separator::new(Orientation::Horizontal));

    let jog_title = Label::new(Some("JOG"));
    jog_title.add_css_class("heading");
    jog_title.set_halign(gtk4::Align::Center);
    panel.append(&jog_title);

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

    let mk_label = |text: &str,
                    h_align: gtk4::Align,
                    v_align: gtk4::Align,
                    margin: (i32, i32, i32, i32)| {
        let lbl = Label::new(Some(text));
        lbl.add_css_class("heading");
        lbl.set_markup(&format!(
            "<span foreground='white' font_weight='bold' size='medium'>{}</span>",
            text
        ));
        lbl.set_halign(h_align);
        lbl.set_valign(v_align);
        lbl.set_margin_start(margin.0);
        lbl.set_margin_end(margin.1);
        lbl.set_margin_top(margin.2);
        lbl.set_margin_bottom(margin.3);
        lbl
    };

    dial_overlay.add_overlay(&mk_label(
        "+Y",
        gtk4::Align::Center,
        gtk4::Align::Start,
        (0, 0, 20, 0),
    ));
    dial_overlay.add_overlay(&mk_label(
        "-Y",
        gtk4::Align::Center,
        gtk4::Align::End,
        (0, 0, 0, 20),
    ));
    dial_overlay.add_overlay(&mk_label(
        "-X",
        gtk4::Align::Start,
        gtk4::Align::Center,
        (18, 0, 0, 0),
    ));
    dial_overlay.add_overlay(&mk_label(
        "+X",
        gtk4::Align::End,
        gtk4::Align::Center,
        (0, 16, 0, 0),
    ));

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

    let scale = Scale::with_range(Orientation::Horizontal, 0.0, 10000.0, 100.0);
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

    let c_cw = controller.clone();
    cw.connect_clicked(move |_| {
        c_cw.lock()
            .unwrap()
            .set_spindle_direction(SpindleDirection::CW)
    });

    let c_ccw = controller.clone();
    ccw.connect_clicked(move |_| {
        c_ccw
            .lock()
            .unwrap()
            .set_spindle_direction(SpindleDirection::CCW)
    });

    let c_stop = controller.clone();
    stop.connect_clicked(move |_| c_stop.lock().unwrap().stop_spindle());

    spin_btns.append(&cw);
    spin_btns.append(&ccw);
    spin_btns.append(&stop);
    spin_box.append(&spin_btns);
    panel.append(&spin_box);

    panel
}

fn create_center_panel(
    _controller: Arc<Mutex<CNCController>>,
    window: ApplicationWindow,
    viewer: Arc<Mutex<GCodeViewer>>,
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
    file_lbl.add_css_class("heading");
    toolbar.append(&file_lbl);

    let start_btn = Button::with_label("▶ Start");
    start_btn.add_css_class("suggested-action");
    toolbar.append(&start_btn);

    for text in ["3D", "Ortho", "Orbit", "Rapid", "Tool"] {
        toolbar.append(&Button::with_label(text));
    }
    panel.append(&toolbar);

    let file_label = file_lbl.clone();
    let viewer_for_dialog = viewer.clone();

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

        dialog.connect_response(move |dialog, response| {
            if response == ResponseType::Accept {
                if let Some(path) = dialog.file().and_then(|file| file.path()) {
                    let filename = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Unknown");
                    label.set_text(filename);

                    let mut viewer = viewer.lock().unwrap();
                    viewer.load_file(&path);
                    viewer.update_draw();
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
