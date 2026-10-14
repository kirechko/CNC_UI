use super::*;

pub(super) fn create_jog_panel(controller: Arc<Mutex<CNCController>>) -> GtkBox {
    let jog_panel = GtkBox::new(Orientation::Vertical, 8);
    let jog_title = Label::new(Some("JOG"));
    jog_title.add_css_class("heading");
    jog_title.set_halign(gtk4::Align::Center);
    jog_panel.append(&jog_title);


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
    jog_panel.append(&jog_container);

    let speed_row = GtkBox::new(Orientation::Horizontal, 8);
    speed_row.set_halign(gtk4::Align::Center);
    speed_row.set_margin_top(4);

    let xy_speed_label = Label::new(Some("XY mm/min"));
    let xy_speed = gtk4::ComboBoxText::new();
    let xy_speeds = [100.0, 300.0, 500.0, 1000.0, 1500.0, 2000.0, 3000.0, 5000.0, 10000.0];
    for speed in xy_speeds {
        let id = speed.to_string();
        let label = format!("{speed:.0}");
        xy_speed.append(Some(&id), &label);
    }
    xy_speed.set_active_id(Some("1000"));
    let xy_speed_controller = controller.clone();
    xy_speed.connect_changed(move |combo| {
        if let Some(speed) = combo.active_id().and_then(|id| id.parse::<f64>().ok()) {
            xy_speed_controller
                .lock()
                .unwrap()
                .set_xy_jog_speed(speed);
        }
    });

    let z_speed_label = Label::new(Some("Z mm/min"));
    let z_speed = gtk4::ComboBoxText::new();
    let z_speeds = [50.0, 100.0, 200.0, 300.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0];
    for speed in z_speeds {
        let id = speed.to_string();
        let label = format!("{speed:.0}");
        z_speed.append(Some(&id), &label);
    }
    z_speed.set_active_id(Some("300"));
    let z_speed_controller = controller.clone();
    z_speed.connect_changed(move |combo| {
        if let Some(speed) = combo.active_id().and_then(|id| id.parse::<f64>().ok()) {
            z_speed_controller
                .lock()
                .unwrap()
                .set_z_jog_speed(speed);
        }
    });

    speed_row.append(&xy_speed_label);
    speed_row.append(&xy_speed);
    speed_row.append(&z_speed_label);
    speed_row.append(&z_speed);
    jog_panel.append(&speed_row);

    jog_panel
}
