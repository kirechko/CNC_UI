use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Box as GtkBox, Button, Label, Orientation, Scale, Grid, Overlay, FileFilter, FileChooserAction, ResponseType};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use crate::cnc_controller::{CNCController, SpindleDirection};
use crate::jog_dial::JogDial;
use crate::gcode_viewer::GCodeViewer;

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

    // Создаем viewer один раз и передаем во все панели
    let viewer = Arc::new(Mutex::new(GCodeViewer::new()));
    
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
    let labels = [("X", "#ff5252"), ("Y", "#69f0ae"), ("Z", "#40c4ff")];
    for (i, (axis, color)) in labels.iter().enumerate() {
        let lbl = Label::new(None);
        lbl.set_markup(&format!("<span foreground='{}' font_weight='bold'>{}</span>", color, axis));
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
    dial_overlay.set_child(Some(&dial.widget));
    
    let mk_label = |text: &str, h_align: gtk4::Align, v_align: gtk4::Align, margin: (i32, i32, i32, i32)| {
        let lbl = Label::new(Some(text));
        lbl.add_css_class("heading"); 
        lbl.set_markup(&format!("<span foreground='white' font_weight='bold' size='medium'>{}</span>", text));
        lbl.set_halign(h_align);
        lbl.set_valign(v_align);
        lbl.set_margin_start(margin.0);
        lbl.set_margin_end(margin.1);
        lbl.set_margin_top(margin.2);
        lbl.set_margin_bottom(margin.3);
        lbl
    };

    dial_overlay.add_overlay(&mk_label("+Y", gtk4::Align::Center, gtk4::Align::Start, (0, 0, 20, 0)));
    dial_overlay.add_overlay(&mk_label("-Y", gtk4::Align::Center, gtk4::Align::End, (0, 0, 0, 20)));
    dial_overlay.add_overlay(&mk_label("-X", gtk4::Align::Start, gtk4::Align::Center, (18, 0, 0, 0)));
    dial_overlay.add_overlay(&mk_label("+X", gtk4::Align::End, gtk4::Align::Center, (0, 16, 0, 0)));

    jog_container.append(&dial_overlay);

    let z_col = GtkBox::new(Orientation::Vertical, 2);
    z_col.set_homogeneous(true);

    let mk_z_btn = |text: &str, axis: Option<char>, dir: Option<i8>, is_step: bool| {
        let btn = Button::with_label(text);
        btn.add_css_class("btn-z");
        
        if let (Some(a), Some(d)) = (axis, dir) {
            let c = controller.clone();
            btn.connect_clicked(move |_| c.lock().unwrap().jog_axis(a, d));
        } else if is_step {
            let step: f64 = text.parse().unwrap_or(1.0);
            let c = controller.clone();
            btn.connect_clicked(move |_| c.lock().unwrap().set_jog_step(step));
        }
        btn
    };

    z_col.append(&mk_z_btn("+Z", Some('Z'), Some(1), false));
    z_col.append(&mk_z_btn("10", None, None, true));
    z_col.append(&mk_z_btn("1", None, None, true));
    z_col.append(&mk_z_btn("0.1", None, None, true));
    z_col.append(&gtk4::Separator::new(Orientation::Horizontal));
    z_col.append(&mk_z_btn("0.1", None, None, true));
    z_col.append(&mk_z_btn("1", None, None, true));
    z_col.append(&mk_z_btn("10", None, None, true));
    z_col.append(&mk_z_btn("-Z", Some('Z'), Some(-1), false));

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
    scale.connect_value_changed(move |s| c_spin.lock().unwrap().set_spindle_speed(s.value()));
    spin_box.append(&scale);
    
    let spin_btns = GtkBox::new(Orientation::Horizontal, 5);
    let cw = Button::with_label("CW");
    let ccw = Button::with_label("CCW");
    let stop = Button::with_label("STOP");
    stop.add_css_class("destructive-action");
    
    let c_cw = controller.clone(); cw.connect_clicked(move |_| c_cw.lock().unwrap().set_spindle_direction(SpindleDirection::CW));
    let c_ccw = controller.clone(); ccw.connect_clicked(move |_| c_ccw.lock().unwrap().set_spindle_direction(SpindleDirection::CCW));
    let c_stop = controller.clone(); stop.connect_clicked(move |_| c_stop.lock().unwrap().stop_spindle());

    spin_btns.append(&cw); spin_btns.append(&ccw); spin_btns.append(&stop);
    spin_box.append(&spin_btns);
    panel.append(&spin_box);

    panel
}

fn create_center_panel(
    _controller: Arc<Mutex<CNCController>>, 
    window: ApplicationWindow,
    viewer: Arc<Mutex<GCodeViewer>>
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

    // Кнопка Start
    let start_btn = Button::with_label("▶ Start");
    start_btn.add_css_class("suggested-action");
    toolbar.append(&start_btn);

    for t in ["3D", "Ortho", "Orbit", "Rapid", "Tool"] {
        let b = Button::with_label(t);
        toolbar.append(&b);
    }
    panel.append(&toolbar);

    // Обработчик загрузки файла
    let file_lbl_clone = file_lbl.clone();
    let viewer_clone = viewer.clone();
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

        let lbl = file_lbl_clone.clone();
        let v = viewer_clone.clone();
        dialog.connect_response(move |dlg, response| {
            if response == ResponseType::Accept {
                if let Some(file) = dlg.file() {
                    if let Some(path) = file.path() {
                        let filename = path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("Unknown");
                        lbl.set_text(filename);
                        
                        // Загружаем в viewer
                        let mut viewer_guard = v.lock().unwrap();
                        viewer_guard.load_file(&path);
                        viewer_guard.update_draw();
                    }
                }
            }
            dlg.destroy();
        });

        dialog.present();
    });

    // Вставляем виджет viewer вместо старой заглушки
    {
        let v = viewer.lock().unwrap();
        panel.append(&v.widget);
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
    let f = Button::with_label("FILES");
    let m = Button::with_label("MACROS");
    tabs.append(&f); 
    tabs.append(&m);
    panel.append(&tabs);

    // Список файлов с возможностью клика для загрузки
    let list = GtkBox::new(Orientation::Vertical, 5);
    let files = [
        "test.nc", 
        "Ray-Gear.nc", 
        "Pocket-Test.nc", 
        "Engrave-Logo.gcode"
    ];
    
    let viewer_clone = viewer.clone();
    for item in &files {
        let lbl = Label::new(Some(item));
        lbl.set_halign(gtk4::Align::Start);
        
        // Делаем лейблы кликабельными
        let gesture = gtk4::GestureClick::new();
        let path_str = item.to_string();
        let v = viewer_clone.clone();
        gesture.connect_released(move |_, _, _, _| {
            let path = PathBuf::from(&path_str);
            if path.exists() {
                let mut vg = v.lock().unwrap();
                vg.load_file(&path);
                vg.update_draw();
                println!("📂 Quick loaded: {}", path_str);
            } else {
                println!("️ File not found: {}", path_str);
            }
        });
        lbl.add_controller(gesture);
        
        list.append(&lbl);
    }
    panel.append(&list);
    panel.append(&gtk4::Separator::new(Orientation::Horizontal));

    let term_title = Label::new(Some("TERMINAL"));
    term_title.add_css_class("heading");
    panel.append(&term_title);

    let term_area = gtk4::TextView::new();
    term_area.set_vexpand(true);
    term_area.set_editable(false);
    panel.append(&term_area);

    panel
}
