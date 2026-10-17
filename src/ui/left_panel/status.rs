 //! Builds the position/status panel and periodically reflects controller telemetry.
#[path = "event_log.rs"]
mod event_log;
#[path = "status_controls.rs"]
mod status_controls;
use super::*;

pub(super) fn create_status_panel(controller: Arc<Mutex<CNCController>>) -> (GtkBox, GtkBox) {
    let pos_box = GtkBox::new(Orientation::Vertical, 4);
    pos_box.add_css_class("position-panel");
    let header = GtkBox::new(Orientation::Horizontal, 10);
    header.set_margin_bottom(2);

    let title = Label::new(Some("POSITION"));
    title.add_css_class("heading");
    title.add_css_class("title-3");
    title.set_halign(gtk4::Align::Start);
    header.append(&title);

    let badge = Label::new(Some("● IDLE"));
    badge.add_css_class("status-badge");
    header.append(&badge);
    pos_box.append(&header);

    let coordinate_row = GtkBox::new(Orientation::Horizontal, 4);
    coordinate_row.add_css_class("position-coordinate-row");
    let coordinate_label = Label::new(Some("COORD"));
    coordinate_label.add_css_class("dim-label");
    coordinate_label.add_css_class("caption");
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

    coordinate_combo.set_hexpand(true);
    coordinate_combo.set_halign(gtk4::Align::End);

    let position_mode = Rc::new(std::cell::Cell::new(false));
    let position_mode_toggle = gtk4::ToggleButton::with_label("WPOS");
    position_mode_toggle.set_tooltip_text(Some("Переключить рабочие и машинные координаты"));
    position_mode_toggle.set_size_request(54, 28);
    let position_mode_for_toggle = position_mode.clone();
    position_mode_toggle.connect_toggled(move |button| {
        let machine_coordinates = button.is_active();
        position_mode_for_toggle.set(machine_coordinates);
        button.set_label(if machine_coordinates { "MPOS" } else { "WPOS" });
    });

    coordinate_row.append(&coordinate_label);
    coordinate_row.append(&coordinate_combo);
    coordinate_row.append(&position_mode_toggle);
    pos_box.append(&coordinate_row);

    let coord_grid = Grid::new();
    coord_grid.add_css_class("position-coordinate-grid");
    coord_grid.set_row_spacing(3);
    coord_grid.set_column_spacing(5);
    coord_grid.set_column_homogeneous(false);

    let position_values = vec![
        Label::new(Some("0.000")),
        Label::new(Some("0.000")),
        Label::new(Some("0.000")),
        Label::new(Some("0.000")),
        Label::new(Some("0.000")),
    ];

    let position_header = Label::new(Some("WPOS"));
    position_header.add_css_class("caption");
    position_header.add_css_class("dim-label");
    position_header.set_halign(gtk4::Align::End);
    coord_grid.attach(&position_header, 1, 0, 1, 1);

    for (i, (axis, color)) in [
        ("X", "#ff5252"),
        ("Y", "#69f0ae"),
        ("Z", crate::theme::ACCENT_BLUE),
        ("A", "#c49be8"),
        ("B", "#e8bd69"),
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
        val.add_css_class("title-3");
        val.add_css_class("position-work-value");
        val.set_halign(gtk4::Align::End);
        val.set_hexpand(true);


        let axis = match *axis {
            "X" => crate::cnc_api::Axis::X,
            "Y" => crate::cnc_api::Axis::Y,
            "Z" => crate::cnc_api::Axis::Z,
            "A" => crate::cnc_api::Axis::A,
            _ => crate::cnc_api::Axis::B,
        };
        let home_button = Button::from_icon_name("go-home-symbolic");
        home_button.set_tooltip_text(Some(&format!("Homing оси {}", axis_label(axis))));
        home_button.set_has_frame(false);
        home_button.set_size_request(24, 24);
        home_button.add_css_class("flat");
        home_button.add_css_class("position-action-button");
        let home_controller = controller.clone();
        home_button.connect_clicked(move |_| home_controller.lock().unwrap().home_axis(axis));

        let zero_button = Button::from_icon_name("crosshair-symbolic");
        zero_button.set_tooltip_text(Some(&format!("Установить ноль оси {}", axis_label(axis))));
        zero_button.set_has_frame(false);
        zero_button.set_size_request(24, 24);
        zero_button.add_css_class("flat");
        zero_button.add_css_class("position-action-button");
        let zero_controller = controller.clone();
        zero_button.connect_clicked(move |_| zero_controller.lock().unwrap().zero_axis(axis));

        coord_grid.attach(&lbl, 0, i as i32 + 1, 1, 1);
        coord_grid.attach(&val, 1, i as i32 + 1, 1, 1);
        coord_grid.attach(&home_button, 2, i as i32 + 1, 1, 1);
        coord_grid.attach(&zero_button, 3, i as i32 + 1, 1, 1);
    }

    pos_box.append(&coord_grid);

    // Обновляем координаты из статуса эмулятора.
    let status_controller = controller.clone();
    let status_values = position_values.clone();
    let status_position_mode = position_mode.clone();
    let status_position_header = position_header.clone();
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
    event_log::connect_log_button(&log_button, event_log.clone(), &pos_box);
    log_row.append(&log_button);
    log_row.append(&latest_event);

    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        update_status_indicators(
            status_controller.clone(),
            status_values.clone(),
            status_position_mode.clone(),
            status_position_header.clone(),
            status_coordinate_combo.clone(),
            status_badge.clone(),
            spindle_status.clone(),
            mist_led_poll.clone(),
            flood_led_poll.clone(),
            event_log_poll.clone(),
            last_event_poll.clone(),
            latest_event_poll.clone(),
        )
    });

    pos_box.append(&status_controls::create_machine_controls(
        controller.clone(),
        &coordinate_combo,
    ));

    (pos_box, log_row)
}

fn axis_label(axis: crate::cnc_api::Axis) -> &'static str {
    match axis {
        crate::cnc_api::Axis::X => "X",
        crate::cnc_api::Axis::Y => "Y",
        crate::cnc_api::Axis::Z => "Z",
        crate::cnc_api::Axis::A => "A",
        crate::cnc_api::Axis::B => "B",
    }
}

fn update_status_indicators(
    status_controller: Arc<Mutex<CNCController>>,
    status_values: Vec<Label>,
    position_mode: Rc<std::cell::Cell<bool>>,
    position_header: Label,
    status_coordinate_combo: gtk4::ComboBoxText,
    status_badge: Label,
    spindle_status: Label,
    mist_led_poll: Label,
    flood_led_poll: Label,
    event_log_poll: Rc<RefCell<Vec<String>>>,
    last_event_poll: Rc<RefCell<Option<String>>>,
    latest_event_poll: Label,
) -> gtk4::glib::ControlFlow {
    let status = status_controller.lock().unwrap().get_status_snapshot();

    let machine_coordinates = position_mode.get();
    position_header.set_text(if machine_coordinates { "MPOS" } else { "WPOS" });

    let coordinates = if machine_coordinates {
        [
            status.machine_x,
            status.machine_y,
            status.machine_z,
            status.machine_a,
            status.machine_b,
        ]
    } else {
        [
            status.x_position,
            status.y_position,
            status.z_position,
            status.a_position,
            status.b_position,
        ]
    };
    for (label, value) in status_values.iter().zip(coordinates) {
        label.set_text(&format!("{value:.3}"));
    }

    let active_id = status.active_coordinate_system.label();
    if status_coordinate_combo.active_id().as_deref() != Some(active_id) {
        status_coordinate_combo.set_active_id(Some(active_id));
    }

    let spindle_text = match status.spindle_direction {
        SpindleDirection::CW => "CW",
        SpindleDirection::CCW => "CCW",
        SpindleDirection::Off => "Stopped",
    };
    let program_progress = if status.total_lines > 0 {
        format!(" · L{}/{}", status.current_line, status.total_lines)
    } else {
        String::new()
    };
    spindle_status.set_text(&format!(
        "F{:.0} · {} {:.0} RPM{}",
        status.feed_rate,
        spindle_text,
        status.spindle_actual_speed,
        program_progress
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

    if let Some((kind, message)) = status.ui_alert.as_ref() {
        let title = match kind.as_str() {
            "connection" => "Нет связи",
            "axis_limit" => "Лимит оси",
            _ => "Команда отклонена",
        };
        status_badge.set_text(&format!("● {title}: {message}"));
        status_badge.add_css_class("alarm-badge");
    } else if status.state == "ALARM" {
        status_badge.set_markup("<span foreground='#ff4d4f'>● ALARM</span>");
        status_badge.add_css_class("alarm-badge");
    } else {
        let display_state = match status.state.as_str() {
            "READING" => "RUNNING",
            "PAUSED" => "PAUSED",
            "IDLE" => "IDLE",
            other => other,
        };
        let color = match display_state {
            "IDLE" => "#64c99a",
            "HOMING" | "RUNNING" => "#42a4d4",
            "PAUSED" => "#e8bd69",
            _ => "#b8c2c5",
        };
        status_badge.set_markup(&format!(
            "<span foreground='{color}'>● {display_state}</span>"
        ));
        status_badge.remove_css_class("alarm-badge");
    }

    gtk4::glib::ControlFlow::Continue
}
