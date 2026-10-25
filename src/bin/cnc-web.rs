//! Runs the Axum web UI without linking the Tauri desktop runtime.
#[path = "../cnc_controller.rs"]
mod cnc_controller;
#[path = "../config.rs"]
mod config;
#[path = "../message_log.rs"]
mod message_log;
#[path = "../web_server.rs"]
mod web_server;

use cnc_core as cnc_api;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    web_server::run().await
}
