 //! Initializes GTK, applies the application theme, and starts the CNC control UI.
#[allow(dead_code)]
mod cnc_api;
mod cnc_controller;
mod adapters;
mod config;
mod grbl_adapter;
#[path = "linuxcnc_api.rs"]
mod em_adapter;
mod web_server;
mod tauri_app;


#[allow(dead_code)] // GTK theme retained while the UI migrates to Tauri.
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

.jog-btn { min-width: 45px; min-height: 45px; font-size: 16px; font-weight: bold; padding: 0; }
.btn-z { min-width: 34px; min-height: 27px; padding: 0;  font-family: inherit; font-size: 14px; font-weight: normal; }
.z-direction { color: #40c4ff; }
.z-gap { min-height: 8px; padding: 0; font-size: 1px; }

button.suggested-action { background-color: #0e639c; color: white; border: none; }
button.suggested-action:hover { background-color: #1177bb; }
button.destructive-action { background-color: #a12626; color: white; border: none; }
button.destructive-action:hover { background-color: #c43030; }

.box-panel { background-color: #252526; border-radius: 8px; padding: 10px; margin: 5px; }
.title-1 { font-size: 20px; font-weight: bold; color: #007acc; }
.status-badge { background-color: #2d2d2d; border: 1px solid #444; border-radius: 4px; padding: 3px 10px; color: #4ec9b0; font-size: 12px; }
separator { background-color: #3c3c3c; min-height: 1px; }

.alarm-badge {
    color: #ff4d4f;
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
    if std::env::args().any(|arg| arg == "--web") {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to initialize web runtime");

        if let Err(error) = runtime.block_on(web_server::run()) {
            eprintln!("Web server failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    if let Err(error) = tauri_app::run() {
        eprintln!("Tauri application failed: {error}");
        std::process::exit(1);
    }
}
