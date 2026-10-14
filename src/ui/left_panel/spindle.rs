 //! Builds spindle-speed and direction controls with alarm-state safeguards.
use super::*;

pub(super) fn create_spindle_controls(controller: Arc<Mutex<CNCController>>) -> GtkBox {
    let spin_box = GtkBox::new(Orientation::Vertical, 5);
    let spin_lbl = Label::new(Some("SPINDLE"));
    spin_lbl.add_css_class("heading");
    spin_lbl.set_halign(gtk4::Align::Center);
    spin_box.append(&spin_lbl);

    let scale = Scale::with_range(Orientation::Horizontal, 0.0, 12000.0, 100.0);
    scale.set_value(0.0);
    let c_spin = controller.clone();
    scale.connect_value_changed(move |s| c_spin.lock().unwrap().set_spindle_speed(s.value()));
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
    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        let alarm = alarm_controller.lock().unwrap().get_status_snapshot().state == "ALARM";
        cw_poll.set_sensitive(!alarm);
        ccw_poll.set_sensitive(!alarm);
        gtk4::glib::ControlFlow::Continue
    });

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
    spin_box
}
