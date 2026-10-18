 //! Starts the CNC emulator, loads configuration, and accepts client connections.
use cnc_core as cnc_api;
#[path = "cnc_emulator/config.rs"]
mod config;
#[path = "cnc_emulator/gcode.rs"]
mod gcode;
#[path = "cnc_emulator/state.rs"]
mod state;
use state::Emulator;
#[path = "cnc_emulator/dynamics.rs"]
mod dynamics;
#[path = "cnc_emulator/protocol.rs"]
mod protocol;

use cnc_api::*;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn default_coordinate_offsets() -> Vec<CoordinateSystemOffset> {
    CoordinateSystem::ALL
        .iter()
        .map(|system| CoordinateSystemOffset {
            system: *system,
            offset: Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                a: 0.0,
                b: 0.0,
            },
        })
        .collect()
}

fn load_coordinate_offsets(path: &Path) -> Result<Vec<CoordinateSystemOffset>, String> {
    if !path.exists() {
        let defaults = default_coordinate_offsets();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }

        let yaml = serde_yaml_ng::to_string(&defaults)
            .map_err(|error| format!("cannot serialize default coordinate offsets: {error}"))?;
        fs::write(path, yaml)
            .map_err(|error| format!("cannot create {}: {error}", path.display()))?;

        println!(
            "Created default coordinate-system offsets at {}",
            path.display()
        );
        return Ok(defaults);
    }

    let content = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut offsets: Vec<CoordinateSystemOffset> = serde_yaml_ng::from_str(&content)
        .map_err(|error| format!("invalid coordinate offsets {}: {error}", path.display()))?;

    for system in CoordinateSystem::ALL {
        if !offsets.iter().any(|entry| entry.system == system) {
            offsets.push(CoordinateSystemOffset {
                system,
                offset: Position {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    a: 0.0,
                    b: 0.0,
                },
            });
        }
    }

    Ok(offsets)
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let socket_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/linuxcnc-ui-emulator.sock"));
    let config_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/emulator.yaml"));

    let config = config::load_config(&config_path).unwrap_or_else(|e| {
        eprintln!("Configuration error: {e}");
        std::process::exit(2);
    });
    let offsets_path = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("coordinate_systems.yaml");
    let coordinate_offsets = load_coordinate_offsets(&offsets_path).unwrap_or_else(|e| {
        eprintln!("Coordinate-system storage error: {e}");
        std::process::exit(2);
    });

    if socket_path.exists() {
        match fs::symlink_metadata(&socket_path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                if UnixStream::connect(&socket_path).is_ok() {
                    eprintln!("An emulator already listens on {}", socket_path.display());
                    std::process::exit(1);
                }
                if let Err(e) = fs::remove_file(&socket_path) {
                    eprintln!("Cannot remove stale socket: {e}");
                    std::process::exit(1);
                }
            }
            Ok(_) => {
                eprintln!(
                    "Refusing to replace non-socket path {}",
                    socket_path.display()
                );
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Cannot inspect socket path: {e}");
                std::process::exit(1);
            }
        }
    }

    let listener = UnixListener::bind(&socket_path).unwrap_or_else(|e| {
        eprintln!("Cannot bind {}: {e}", socket_path.display());
        std::process::exit(1);
    });

    println!("Loaded configuration from {}", config_path.display());
    println!("CNC emulator listening on {}", socket_path.display());

    let state = Arc::new(Mutex::new(Emulator::new(
        config,
        offsets_path,
        coordinate_offsets,
    )));
    let ticker = state.clone();
    thread::spawn(move || {
        let period = Duration::from_millis(10);
        loop {
            thread::sleep(period);
            if let Ok(mut emulator) = ticker.lock() {
                let now = Instant::now();
                let dt = now
                    .duration_since(emulator.last_tick)
                    .as_secs_f64()
                    .clamp(0.001, 0.05);
                emulator.last_tick = now;
                emulator.tick(dt);
            }
        }
    });

    for connection in listener.incoming() {
        match connection {
            Ok(stream) => {
                let state = state.clone();
                thread::spawn(move || protocol::serve_client(stream, state));
            }
            Err(e) => eprintln!("Accept error: {e}"),
        }
    }
}
