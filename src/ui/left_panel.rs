mod status;
mod jog;
mod spindle;
use super::*;

pub(super) fn create_left_panel(controller: Arc<Mutex<CNCController>>) -> (GtkBox, GtkBox) {
    let panel = GtkBox::new(Orientation::Vertical, 10);
    panel.add_css_class("box-panel");
    panel.set_vexpand(true);
    panel.set_hexpand(false);
    panel.set_size_request(340, -1);

    let (pos_box, log_row) = status::create_status_panel(controller.clone());
    panel.append(&pos_box);
    panel.append(&gtk4::Separator::new(Orientation::Horizontal));
    panel.append(&gtk4::Separator::new(Orientation::Horizontal));

    panel.append(&jog::create_jog_panel(controller.clone()));
    panel.append(&gtk4::Separator::new(Orientation::Horizontal));

    panel.append(&spindle::create_spindle_controls(controller.clone()));

    (panel, log_row)
}
