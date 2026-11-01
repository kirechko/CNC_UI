//! Compatibility entry point for running the CNC emulator by itself.
use cnc_core as cnc_api;
#[path = "../emulator.rs"]
mod emulator;
#[path = "../message_log.rs"]
mod message_log;

/// Starts the standalone emulator process and reports startup failures.
fn main() {
    let mut args = std::env::args_os().skip(1);
    let address = args
        .next()
        .and_then(|value| value.into_string().ok())
        .or_else(|| std::env::var("CNC_EMULATOR_ADDRESS").ok())
        .unwrap_or_else(|| "127.0.0.1:8765".to_string());
    let config_path = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("config/emulator.yaml"));
    let result =
        emulator::EmulatorHandle::start(address, config_path).and_then(|worker| worker.join());
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
