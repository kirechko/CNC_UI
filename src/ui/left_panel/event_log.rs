 //! Displays the controller message history in a separate dialog.
use super::*;

pub(super) fn connect_log_button(
    button: &Button,
    history: Rc<RefCell<Vec<String>>>,
    parent: &GtkBox,
) {
    let history = history.clone();
    let parent = parent.clone();
    button.connect_clicked(move |_| {
        let dialog = gtk4::Window::builder()
            .title("Controller event log")
            .default_width(600)
            .default_height(400)
            .transient_for(
                &parent
                    .root()
                    .and_then(|root| root.downcast::<ApplicationWindow>().ok())
                    .expect("event log parent window"),
            )
            .modal(true)
            .build();

        let content = GtkBox::new(Orientation::Vertical, 6);
        content.set_margin_start(10);
        content.set_margin_end(10);
        content.set_margin_top(10);
        content.set_margin_bottom(10);

        let view = gtk4::TextView::new();
        view.set_editable(false);
        view.set_cursor_visible(false);
        view.set_monospace(true);
        let buffer = view.buffer();
        let entries = history.borrow().join("\n");
        buffer.set_text(&entries);

        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_child(Some(&view));
        content.append(&scroll);

        let close = Button::with_label("Close");
        let dialog_clone = dialog.clone();
        close.connect_clicked(move |_| dialog_clone.close());
        content.append(&close);

        dialog.set_child(Some(&content));
        dialog.present();
    });
}
