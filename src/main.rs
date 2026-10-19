//! Initializes GTK, applies the application theme, and starts the CNC control UI.
use cnc_core as cnc_api;
mod cnc_controller;
mod config;
mod message_log;
mod tauri_app;
mod web_server;


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
