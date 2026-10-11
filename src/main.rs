mod cnc_controller;
#[allow(dead_code)]
mod cnc_api;
mod linuxcnc_api;
mod ui;
mod jog_dial;
mod gcode_viewer;
mod theme; // <-- НОВЫЙ МОДУЛЬ

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
.btn-z { min-width: 34px !important; min-height: 27px !important; padding: 0 !important;  font-family: inherit; font-size: 14px !important; font-weight: normal; }
.z-direction { color: #40c4ff !important; }
.z-gap { min-height: 8px !important; padding: 0 !important; font-size: 1px !important; }

button.suggested-action { background-color: #0e639c; color: white; border: none; }
button.suggested-action:hover { background-color: #1177bb; }
button.destructive-action { background-color: #a12626; color: white; border: none; }
button.destructive-action:hover { background-color: #c43030; }

.box-panel { background-color: #252526; border-radius: 8px; padding: 10px; margin: 5px; }
.title-1 { font-size: 20px; font-weight: bold; color: #007acc; }
.status-badge { background-color: #2d2d2d; border: 1px solid #444; border-radius: 4px; padding: 3px 10px; color: #4ec9b0; font-size: 12px; }
separator { background-color: #3c3c3c; min-height: 1px; }

.alarm-badge {
    color: #ff4d4f !important;
    font-weight: bold;
}

.coolant-led {
    color: #b8c7e0;
    background-color: #10234a;
    border: 1px solid #233c6a;
    border-radius: 5px;
    padding: 3px 8px;
    font-size: 10px;
    font-weight: bold;
}
.coolant-led-on {
    color: #ffffff;
    background-color: #1677ff;
    border-color: #62a0ff;
}
.event-log-latest { font-size: 11px; }
"#;

fn main() {
    gtk4::init().expect("Failed to initialize GTK");
    if let Some(settings) = gtk4::Settings::default() {
        settings.set_property("gtk-application-prefer-dark-theme", true);
    }

    let provider = gtk4::CssProvider::new();
    provider.load_from_data(DARK_THEME_CSS);

    let palette_provider = gtk4::CssProvider::new();
    let palette_css = format!(
        "@define-color cnc-accent {};\
         @define-color cnc-accent-hover {};\
         button.suggested-action {{ background-color: @cnc-accent; color: white; }}\
         button.suggested-action:hover {{ background-color: @cnc-accent-hover; }}",
        theme::ACCENT_BLUE,
        theme::ACCENT_BLUE_HOVER
    );
    palette_provider.load_from_data(&palette_css);
    
    if let Some(display) = gtk4::gdk::Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk4::style_context_add_provider_for_display(
            &display,
            &palette_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }

    let app = gtk4::Application::builder()
        .application_id("com.kirechko.fluidnc-rust")
        .build();

    let socket_path = std::env::var_os("CNC_EMULATOR_SOCKET")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/linuxcnc-ui-emulator.sock"));

    let controller = Arc::new(Mutex::new(
        CNCController::connect(&socket_path).unwrap_or_else(|error| {
            eprintln!(
                "Не удалось подключиться к эмулятору {}: {}: {}",
                socket_path.display(),
                error.code,
                error.message
            );
            eprintln!("Сначала запустите отдельный процесс cnc-emulator.");
            std::process::exit(1);
        }),
    ));

    app.connect_activate(move |app| {
        ui::build_ui(app, controller.clone());
    });

    app.run();
}
