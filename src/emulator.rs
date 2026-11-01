//! Starts the CNC emulator, loads configuration, and accepts client connections.
use crate::cnc_api;
#[path = "bin/cnc_emulator/config.rs"]
mod config;
#[path = "bin/cnc_emulator/gcode.rs"]
mod gcode;
#[path = "message_log.rs"]
mod message_log;
#[path = "bin/cnc_emulator/state.rs"]
mod state;
use state::Emulator;
#[path = "bin/cnc_emulator/dynamics.rs"]
mod dynamics;
#[path = "bin/cnc_emulator/protocol.rs"]
mod protocol;

use cnc_api::*;
use std::fs;
use std::net::TcpListener;
use std::path::Path;
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
                c: 0.0,
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
                    c: 0.0,
                },
            });
        }
    }

    Ok(offsets)
}

/// Owns the emulator worker and guarantees its thread is joined during shutdown.
pub struct EmulatorHandle {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<thread::JoinHandle<Result<(), String>>>,
}

impl EmulatorHandle {
    /// Starts the emulator, returning only after configuration and listener setup succeed.
    pub fn start(address: String, config_path: std::path::PathBuf) -> Result<Self, String> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("cnc-emulator".into())
            .spawn(move || {
                let mut ready_tx = Some(ready_tx);
                let result = run_at_with_ready(
                    &address,
                    &config_path,
                    || worker_stop.load(std::sync::atomic::Ordering::Acquire),
                    || {
                        if let Some(sender) = ready_tx.take() {
                            let _ = sender.send(Ok(()));
                        }
                    },
                );
                if let (Some(sender), Err(error)) = (ready_tx, &result) {
                    let _ = sender.send(Err(error.clone()));
                }
                result
            })
            .map_err(|error| format!("Cannot start emulator worker: {error}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(error) => {
                let _ = thread.join();
                Err(format!(
                    "Emulator worker exited before becoming ready: {error}"
                ))
            }
        }
    }

    /// Signals shutdown and joins the worker, returning any worker failure.
    pub fn shutdown(mut self) -> Result<(), String> {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.join_worker()
    }

    /// Waits for the worker thread after its stop token has been raised.
    fn join_worker(&mut self) -> Result<(), String> {
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .map_err(|_| "Emulator worker panicked".to_string())?,
            None => Ok(()),
        }
    }
}

impl Drop for EmulatorHandle {
    /// Requests shutdown and prevents the worker from being detached on early returns.
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        let _ = self.join_worker();
    }
}

/// Starts the standalone emulator using its conventional environment and config defaults.
pub fn run() -> Result<(), String> {
    let address =
        std::env::var("CNC_EMULATOR_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8765".to_string());
    run_at(&address, Path::new("config/emulator.yaml"), || false)
}

/// Runs the emulator until the supplied stop predicate reports shutdown.
pub fn run_at(
    address: &str,
    config_path: &Path,
    should_stop: impl Fn() -> bool,
) -> Result<(), String> {
    run_at_with_ready(address, config_path, should_stop, || {})
}

/// Runs the emulator and notifies the owner after its configuration and listener are ready.
fn run_at_with_ready(
    address: &str,
    config_path: &Path,
    should_stop: impl Fn() -> bool,
    on_ready: impl FnOnce(),
) -> Result<(), String> {
    let config = config::load_config(config_path)
        .map_err(|error| format!("Configuration error: {error}"))?;
    let offsets_path = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("coordinate_systems.yaml");
    let coordinate_offsets = load_coordinate_offsets(&offsets_path)
        .map_err(|error| format!("Coordinate-system storage error: {error}"))?;
    let listener = TcpListener::bind(address)
        .map_err(|error| format!("Cannot bind emulator to {address}: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Cannot configure emulator listener: {error}"))?;

    println!("Loaded configuration from {}", config_path.display());
    println!("CNC emulator listening on {address}");
    on_ready();
    let state = Arc::new(Mutex::new(Emulator::new(
        config,
        offsets_path,
        coordinate_offsets,
    )));
    let ticker = state.clone();
    let stop_clients = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ticker_stop = stop_clients.clone();
    let ticker_thread = thread::spawn(move || {
        let period = Duration::from_millis(10);
        while !ticker_stop.load(std::sync::atomic::Ordering::Acquire) {
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
    let mut clients: Vec<thread::JoinHandle<()>> = Vec::new();

    while !should_stop() {
        let mut index = 0;
        while index < clients.len() {
            if clients[index].is_finished() {
                let finished = clients.swap_remove(index);
                let _ = finished.join();
            } else {
                index += 1;
            }
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let state = state.clone();
                let client_stop = stop_clients.clone();
                clients.push(thread::spawn(move || {
                    protocol::serve_client(stream, state, client_stop)
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => eprintln!("Accept error: {error}"),
        }
    }

    stop_clients.store(true, std::sync::atomic::Ordering::Release);
    for client in clients {
        let _ = client.join();
    }
    let _ = ticker_thread.join();
    Ok(())
}

#[cfg(test)]
mod worker_tests {
    use super::EmulatorHandle;
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    /// Creates a unique temporary config directory and removes it when dropped.
    struct TestConfig(PathBuf);

    impl TestConfig {
        /// Writes the checked-in emulator defaults into an isolated test directory.
        fn create() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("cnc-ui-emulator-test-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).expect("create test config directory");
            fs::write(
                path.join("emulator.yaml"),
                include_str!("../config/emulator.yaml"),
            )
            .expect("write test emulator config");
            Self(path)
        }

        /// Returns the emulator configuration path inside this temporary directory.
        fn config_path(&self) -> PathBuf {
            self.0.join("emulator.yaml")
        }
    }

    impl Drop for TestConfig {
        /// Removes generated test files after worker threads have stopped.
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Allocates a currently unused IPv4 loopback endpoint for an emulator test.
    fn unused_address() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        listener.local_addr().unwrap().to_string()
    }

    /// Starts and stops a worker while a protocol client remains connected and idle.
    #[test]
    fn worker_shutdown_closes_listener_and_idle_client() {
        let config = TestConfig::create();
        let address = unused_address();
        let mut client = None;
        let handle = EmulatorHandle::start(address.clone(), config.config_path())
            .expect("start emulator worker");
        let socket_address = address.to_socket_addrs().unwrap().next().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(mut stream) =
                TcpStream::connect_timeout(&socket_address, Duration::from_millis(50))
            {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("set response timeout");
                stream
                    .write_all(b"{\"method\":\"status\"}\n")
                    .expect("send status request");
                let mut response = String::new();
                BufReader::new(stream.try_clone().expect("clone test socket"))
                    .read_line(&mut response)
                    .expect("read status response");
                assert!(
                    response.contains("\"ok\":true"),
                    "unexpected response: {response}"
                );
                client = Some(stream);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            client.is_some(),
            "emulator should accept and process a protocol request"
        );

        handle.shutdown().expect("stop emulator worker");
        drop(client);
        assert!(
            TcpStream::connect_timeout(&socket_address, Duration::from_millis(100)).is_err(),
            "listener should be closed after worker shutdown"
        );
    }

    /// Returns bind errors to the caller instead of silently running a partial worker.
    #[test]
    fn worker_reports_listener_bind_failure() {
        let config = TestConfig::create();
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
        let address = occupied.local_addr().unwrap().to_string();
        let error = EmulatorHandle::start(address, config.config_path())
            .err()
            .expect("occupied address should fail");
        assert!(
            error.contains("Cannot bind emulator"),
            "unexpected error: {error}"
        );
    }
}
