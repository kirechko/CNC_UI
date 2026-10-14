#[path = "event_log.rs"]
mod event_log;
#[path = "status_controls.rs"]
mod status_controls;
use super::*;

pub(super) fn create_status_panel(controller: Arc<Mutex<CNCController>>) -> (GtkBox, GtkBox) {
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
    event_log::connect_log_button(&log_button, event_log.clone(), &pos_box);
    log_row.append(&log_button);
    log_row.append(&latest_event);

    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        update_status_indicators(
            status_controller.clone(),
            status_values.clone(),
            status_machine_values.clone(),
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

fn update_status_indicators(
    status_controller: Arc<Mutex<CNCController>>,
    status_values: Vec<Label>,
    status_machine_values: Vec<Label>,
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
        spindle_text, status.spindle_actual_speed
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
        status_badge.set_text(&format!("● {}", display_state));
        status_badge.remove_css_class("alarm-badge");
    }

    gtk4::glib::ControlFlow::Continue
}
