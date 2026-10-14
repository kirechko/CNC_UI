use super::*;

pub(super) fn create_center_panel(
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
            eprintln!(
                "[LinuxCncApi] pause/resume: {}: {}",
                error.code, error.message
            );
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
    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        let status = state_controller.lock().unwrap().get_status_snapshot();

        if status.state == "READING" || status.state == "PAUSED" {
            state_stack.set_visible_child_name("running");
            show_play_icon.set(status.state == "PAUSED");
            state_pause_icon_area.queue_draw();
        } else {
            state_stack.set_visible_child_name("stopped");
        }

        gtk4::glib::ControlFlow::Continue
    });

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

    let fit_button = Button::from_icon_name("zoom-fit-best-symbolic");
    fit_button.set_tooltip_text(Some("Fit trajectory to view"));
    let fit_viewer = viewer.clone();
    fit_button.connect_clicked(move |_| {
        fit_viewer.lock().unwrap().fit_to_trajectory();
    });
    toolbar.append(&fit_button);

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
                        eprintln!(
                            "[LinuxCncApi] load program: {}: {}",
                            error.code, error.message
                        );
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
