 //! Builds machine-wide zero, homing, alarm-reset, and emergency-stop controls.
use super::*;

pub(super) fn create_machine_controls(
    controller: Arc<Mutex<CNCController>>,
    coordinate_combo: &gtk4::ComboBoxText,
) -> GtkBox {
    let coordinate_combo = coordinate_combo.clone();
    let sys_btns = GtkBox::new(Orientation::Horizontal, 5);
    let zero_btn = Button::with_label("Zero All");
    zero_btn.add_css_class("suggested-action");
    let zero_controller = controller.clone();
    let zero_system_combo = coordinate_combo.clone();
    zero_btn.connect_clicked(move |_| {
        let Some(id) = zero_system_combo.active_id() else {
            return;
        };
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
    sys_btns
}
