//! End-to-end regression tests that launch the standalone emulator binary and use its TCP JSON-lines API.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Isolated emulator configuration directory owned by one integration test.
struct TestDirectory(PathBuf);

impl TestDirectory {
    /// Copies a selected repository emulator configuration to an isolated temporary directory.
    fn create_with_config(config: &str) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cnc-ui-emulator-integration-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create isolated emulator directory");
        fs::write(path.join("emulator.yaml"), config).expect("copy emulator configuration");
        Self(path)
    }

    /// Copies the default conventional-spindle profile for existing integration scenarios.
    fn create() -> Self {
        Self::create_with_config(include_str!("../config/emulator.yaml"))
    }

    /// Returns the configuration file path passed to the child process.
    fn config_path(&self) -> PathBuf {
        self.0.join("emulator.yaml")
    }
}

impl Drop for TestDirectory {
    /// Removes temporary configuration and coordinate-offset files after the child exits.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Child-process guard that terminates the emulator if a test assertion panics.
struct EmulatorProcess(Child);

impl EmulatorProcess {
    /// Spawns the actual `cnc-emulator` binary on an available loopback port.
    fn start(config_path: &PathBuf) -> (Self, String) {
        let reservation =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve an ephemeral loopback port");
        let address = reservation.local_addr().unwrap().to_string();
        drop(reservation);
        let child = Command::new(env!("CARGO_BIN_EXE_cnc-emulator"))
            .arg(&address)
            .arg(config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start standalone emulator binary");
        (Self(child), address)
    }
}

impl Drop for EmulatorProcess {
    /// Stops and reaps the test-owned child process, including when an assertion panics.
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits for the emulator TCP listener and returns a buffered protocol connection.
fn connect(address: &str) -> BufReader<TcpStream> {
    let socket_address = address.to_socket_addrs().unwrap().next().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(stream) = TcpStream::connect_timeout(&socket_address, Duration::from_millis(100))
        {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set emulator response timeout");
            return BufReader::new(stream);
        }
        assert!(Instant::now() < deadline, "emulator listener did not start");
        thread::sleep(Duration::from_millis(10));
    }
}

/// Sends one JSON-lines API request and returns the decoded response envelope.
fn request(reader: &mut BufReader<TcpStream>, method: &str, args: Value) -> Value {
    let encoded = json!({"method": method, "args": args}).to_string();
    let stream = reader.get_mut();
    stream
        .write_all(encoded.as_bytes())
        .expect("write emulator request");
    stream.write_all(b"\n").expect("finish request line");
    stream.flush().expect("flush emulator request");
    let mut response = String::new();
    reader
        .read_line(&mut response)
        .expect("read emulator response");
    serde_json::from_str(&response).expect("decode emulator response")
}

/// Verifies a standalone laser profile publishes consistent configuration and safely handles S/M words.
#[test]
fn standalone_emulator_laser_profile_reports_and_controls_output_over_tcp() {
    let directory =
        TestDirectory::create_with_config(include_str!("../config/emulator-laser.yaml"));
    let (_process, address) = EmulatorProcess::start(&directory.config_path());
    let mut reader = connect(&address);

    let configuration = request(&mut reader, "machine_configuration", json!({}));
    assert_eq!(
        configuration["ok"], true,
        "configuration response: {configuration}"
    );
    let spindle = &configuration["result"]["spindle"];
    assert_eq!(spindle["mode"], "laser_constant");
    assert_eq!(spindle["laser"]["min_power"], 0.0);
    assert_eq!(spindle["laser"]["max_power"], 255.0);
    assert_eq!(spindle["max_rpm"], 255.0);

    let initial_status = request(&mut reader, "status", json!({}));
    assert_eq!(
        initial_status["result"]["spindle"]["mode"],
        "laser_constant"
    );
    assert_eq!(initial_status["result"]["spindle"]["laser_power"], 0.0);
    assert_eq!(initial_status["result"]["spindle"]["enabled"], false);

    for (method, args) in [
        ("power_on", json!({})),
        ("mdi", json!({"command": "S40 M3"})),
    ] {
        let response = request(&mut reader, method, args);
        assert_eq!(response["ok"], true, "{method} response: {response}");
    }
    let emitting = request(&mut reader, "status", json!({}));
    assert_eq!(emitting["result"]["spindle"]["enabled"], true);
    assert_eq!(emitting["result"]["spindle"]["laser_power"], 40.0);

    let stopped = request(&mut reader, "mdi", json!({"command": "M5"}));
    assert_eq!(stopped["ok"], true, "M5 response: {stopped}");
    let stopped_status = request(&mut reader, "status", json!({}));
    assert_eq!(stopped_status["result"]["spindle"]["enabled"], false);
    assert_eq!(stopped_status["result"]["spindle"]["laser_power"], 0.0);

    let dynamic = request(&mut reader, "mdi", json!({"command": "S80 M4"}));
    assert_eq!(dynamic["ok"], true, "M4 response: {dynamic}");
    let dynamic_status = request(&mut reader, "status", json!({}));
    assert_eq!(dynamic_status["result"]["spindle"]["mode"], "laser_dynamic");
    assert_eq!(dynamic_status["result"]["spindle"]["laser_power"], 80.0);
    assert_eq!(dynamic_status["result"]["spindle"]["enabled"], true);

    let stopped = request(&mut reader, "mdi", json!({"command": "M5"}));
    assert_eq!(stopped["ok"], true, "second M5 response: {stopped}");
    let over_limit = request(&mut reader, "mdi", json!({"command": "S256 M3"}));
    assert_eq!(
        over_limit["ok"], false,
        "out-of-range S response: {over_limit}"
    );
    let final_status = request(&mut reader, "status", json!({}));
    assert_eq!(final_status["result"]["spindle"]["enabled"], false);
    assert_eq!(final_status["result"]["spindle"]["laser_power"], 0.0);
}

/// Exercises load, confirmed run command, real motion, line progress, and completion over TCP.
#[test]
fn standalone_emulator_runs_a_loaded_program_over_tcp() {
    let directory = TestDirectory::create();
    let (_process, address) = EmulatorProcess::start(&directory.config_path());
    let mut reader = connect(&address);

    let loaded = request(
        &mut reader,
        "load_program",
        json!({"name": "integration.nc", "content": "G21\nG90\nG1 X5 F600\nM30\n"}),
    );
    assert_eq!(loaded["ok"], true, "load response: {loaded}");
    let started = request(&mut reader, "program_run", json!({"start_line": 0}));
    assert_eq!(started["ok"], true, "run response: {started}");

    let deadline = Instant::now() + Duration::from_secs(5);
    let final_status = loop {
        let response = request(&mut reader, "status", json!({}));
        assert_eq!(response["ok"], true, "status response: {response}");
        let status = response["result"].clone();
        if status["interpreter_state"] == "Idle" && status["current_line"].as_u64() == Some(4) {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "program did not finish: {status}"
        );
        thread::sleep(Duration::from_millis(15));
    };

    assert_eq!(final_status["position"]["x"], 5.0);
    assert_eq!(final_status["total_lines"], 4);
    assert_eq!(final_status["message"], "Program finished");
}

/// Verifies hold-to-jog has bounded monotonic increments and a gradual release ramp over TCP.
#[test]
fn standalone_emulator_continuous_jog_is_smooth_and_stops_with_deceleration() {
    let directory = TestDirectory::create();
    let (_process, address) = EmulatorProcess::start(&directory.config_path());
    let mut reader = connect(&address);
    let started = request(
        &mut reader,
        "jog_continuous",
        json!({"axis": "X", "direction": 1, "feed_rate": 1200.0}),
    );
    assert_eq!(started["ok"], true, "jog start response: {started}");

    let mut previous_x = 0.0;
    let mut deltas = Vec::new();
    for _ in 0..50 {
        let response = request(&mut reader, "status", json!({}));
        assert_eq!(response["ok"], true, "jog status response: {response}");
        let status = &response["result"];
        let x = status["position"]["x"].as_f64().unwrap();
        let delta = x - previous_x;
        assert!(delta >= -1e-9, "continuous jog reversed: {delta}");
        if delta > 1e-9 {
            deltas.push(delta);
        }
        previous_x = x;
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        deltas.len() >= 10,
        "jog should advance during hold: {deltas:?}"
    );
    assert!(
        deltas[0] < deltas[deltas.len() - 1],
        "jog should ramp up: {deltas:?}"
    );

    let stopped = request(
        &mut reader,
        "jog_continuous",
        json!({"axis": "X", "direction": 0, "feed_rate": 1200.0}),
    );
    assert_eq!(stopped["ok"], true, "jog stop response: {stopped}");
    let stop_start_x = previous_x;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut last_x = previous_x;
    let mut saw_final_stop = false;
    while Instant::now() < deadline {
        let response = request(&mut reader, "status", json!({}));
        let status = &response["result"];
        let x = status["position"]["x"].as_f64().unwrap();
        assert!(x + 1e-9 >= last_x, "release caused a position reversal");
        last_x = x;
        if status["motion_mode"].is_null() && status["jog_rate"].as_f64() == Some(0.0) {
            saw_final_stop = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(saw_final_stop, "jog did not stop before deadline");
    assert!(
        last_x > stop_start_x,
        "release should allow deceleration travel"
    );
}
