//! Runs the Axum web UI without linking the Tauri desktop runtime.
// Directory module (mod.rs owns the CNCController facade; helpers live in sibling files).
#[path = "../cnc_controller/mod.rs"]
mod cnc_controller;
#[path = "../i18n.rs"]
mod i18n;
#[path = "../message_log.rs"]
mod message_log;
// Directory module (mod.rs re-exports `run` and the shared models).
#[path = "../web_server/mod.rs"]
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
