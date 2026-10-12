use super::*;

pub(super) fn create_right_panel(viewer: Arc<Mutex<GCodeViewer>>) -> GtkBox {
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
