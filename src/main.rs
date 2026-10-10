mod cnc_controller;
mod ui;
mod jog_dial;
mod gcode_viewer; // <-- НОВЫЙ МОДУЛЬ

use gtk4::prelude::*;
use std::sync::{Arc, Mutex};
use cnc_controller::CNCController;

const DARK_THEME_CSS: &str = r#"
window { 
    background-color: #1e1e1e; 
    color: #cccccc; 
    font-family: 'Roboto', 'Segoe UI', 'Helvetica Neue', Arial, sans-serif;
}

.heading { 
    font-size: 13px; 
    color: #858585; 
    text-transform: uppercase; 
    letter-spacing: 1.5px;
    font-weight: 600;
}

.monospace { 
    font-family: 'JetBrains Mono', 'Consolas', 'Courier New', monospace; 
    font-size: 16px; 
    font-variant-numeric: tabular-nums;
}

button { 
    all: unset;
    background-color: #3c3c3c;
    color: #ffffff;
    border: 1px solid #555;
    border-radius: 4px;
    padding: 6px 10px;
    font-family: inherit;
    font-size: 14px;
    transition: background-color 0.1s ease;
}

button:hover { background-color: #4a4a4a; border-color: #666; }
button:active, button:checked { background-color: #2d2d2d; border-color: #007acc; }

.jog-btn { min-width: 45px !important; min-height: 45px !important; font-size: 16px !important; font-weight: bold; padding: 0 !important; }
.btn-z { min-width: 40px !important; min-height: 38px !important; font-size: 14px !important; font-weight: bold; padding: 0 !important; }

button.suggested-action { background-color: #0e639c; color: white; border: none; }
button.suggested-action:hover { background-color: #1177bb; }
button.destructive-action { background-color: #a12626; color: white; border: none; }
button.destructive-action:hover { background-color: #c43030; }

.box-panel { background-color: #252526; border-radius: 8px; padding: 10px; margin: 5px; }
.title-1 { font-size: 20px; font-weight: bold; color: #007acc; }
.status-badge { background-color: #2d2d2d; border: 1px solid #444; border-radius: 4px; padding: 3px 10px; color: #4ec9b0; font-size: 12px; }
separator { background-color: #3c3c3c; min-height: 1px; }
"#;

fn main() {
    gtk4::init().expect("Failed to initialize GTK");
    if let Some(settings) = gtk4::Settings::default() {
        settings.set_property("gtk-application-prefer-dark-theme", true);
    }

    let provider = gtk4::CssProvider::new();
    provider.load_from_data(DARK_THEME_CSS);
    
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let app = gtk4::Application::builder()
        .application_id("com.kirechko.fluidnc-rust")
        .build();

    let controller = Arc::new(Mutex::new(CNCController::new()));

    app.connect_activate(move |app| {
        ui::build_ui(app, controller.clone());
    });

    app.run();
}
