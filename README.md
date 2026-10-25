# CNC UI

A CNC control application built with Rust and Tauri. It provides a desktop interface and an optional browser-based web interface, plus a standalone software emulator for development and testing without a physical machine.

The UI communicates with the selected backend through a project-specific CNC API. The emulator uses a local Unix-domain socket and a JSON protocol inspired by LinuxCNC; it is **not NML-compatible** and is not a replacement for a LinuxCNC controller.

## Features

- Tauri desktop application and optional browser-based web interface
- XY and Z jog controls, including incremental and continuous movement
- G-code loading, toolpath visualization, and program controls
- Position readouts, work-coordinate selection, zeroing, and homing controls
- Spindle speed and direction controls with telemetry display
- Standalone emulator with simulated motion, spindle, coolant, homing, and soft limits
- GRBL 1.1 serial adapter
- YAML configuration for backend selection, connection settings, axis limits, and emulator dynamics

## Requirements

- Linux
- Rust toolchain and Cargo
- Tauri's Linux system dependencies, including the WebKitGTK development libraries

Install the prerequisites for your distribution using the [Tauri prerequisites guide](https://v2.tauri.app/start/prerequisites/).

## Build

Run these commands from the repository root:

```bash
cargo check --all-targets
cargo build --bin cnc-emulator
cargo build --bin linuxcnc-ui
cargo build --bin cnc-web
```

## Run

Start the emulator in one terminal from the repository root:

```bash
cargo run --bin cnc-emulator
```

By default, the emulator reads `config/emulator.yaml` and listens on:

```text
/tmp/linuxcnc-ui-emulator.sock
```

Start the UI in a second terminal:

```bash
cargo run --bin linuxcnc-ui
```

The emulator must be running before the desktop application starts.

To run the browser-based web interface instead, start the emulator and run:

```bash
cargo run --bin linuxcnc-ui -- --web
```

The web server listens on `0.0.0.0:8080`; open `http://localhost:8080` in a browser. The server has no authentication, so use it only on a trusted network.

The standalone web backend binary `cnc-web` runs the same web interface without linking the Tauri desktop runtime:

```bash
CNC_UI_CONFIG=config/fluidnc-test.yaml RUST_LOG=info cargo run --bin cnc-web
```

Logs are written to stderr through `tracing`; `RUST_LOG` controls the verbosity (default `info`).

To use a different emulator socket path for the UI, set `CNC_EMULATOR_SOCKET`:

```bash
CNC_EMULATOR_SOCKET=/tmp/my-cnc.sock cargo run --bin linuxcnc-ui
```

The emulator accepts an optional socket path followed by an optional configuration-file path:

```bash
cargo run --bin cnc-emulator -- /tmp/my-cnc.sock config/emulator.yaml
```

## Architecture

The desktop and web interfaces share the browser-based UI in `web/index.html`. The Tauri application embeds this interface; the optional web mode serves it through Axum.

UI commands are handled by `CNCController`, which queues commands on a worker thread and maintains a cached status snapshot. The worker communicates through the shared `LinuxCncApi` interface. The emulator runs as a separate process; `EmAdapter` connects to it over a Unix-domain socket and translates API calls to the emulator's JSON protocol.

## Backend configuration

The application reads `cnc-ui.yaml` from the current working directory by default. Set `CNC_UI_CONFIG` to use a different configuration file. Select one backend with the `backend` setting:

- `emulator` — connects to the standalone emulator. Its socket path can be configured in `emulator.socket_path`; `CNC_EMULATOR_SOCKET` can override it for the UI.
- `grbl` — connects to a GRBL 1.1 controller over the configured serial port and baud rate. The user running the application must have permission to access the device.
- `fluid_nc_web_socket` — connects to an ESP32 FluidNC controller over WebSocket (port 80). The address is found by automatic discovery; there is no manual address setting. See [FluidNC over WebSocket](#fluidnc-over-websocket).
- `fluid_nc_rs232` — uses the GRBL-compatible serial adapter for FluidNC over the configured `fluidnc.serial_port` and `fluidnc.baud_rate`. Firmware compatibility is not verified on hardware.
- `linux_cnc` and `fluid_nc` — debug placeholders. They log their parameters and do not connect to hardware.

The `linuxcnc.socket_path` setting is a placeholder for a future adapter.

## FluidNC over WebSocket

The `fluid_nc_web_socket` adapter works as follows:

- Discovery scans the IPv4 networks of local interfaces (no wider than `/24`) and any hosts or CIDRs listed in `fluidnc.discovery_port`. Each candidate is probed on WebSocket (TCP 80) and Telnet (TCP 23). A device is accepted only after a read-only `$I` response confirms FluidNC.
- Cached endpoints are probed first. The adapter connects to the first machine found and keeps it selected; other machines are listed for reference only.
- After connecting, the adapter reads `$ESP130`, `$ESP131`, `$ESP120`, and `$ESP121`. When `$ESP120=ON`, the WebSocket endpoint is added to the machine table on the port reported by `$ESP121`. FluidNC uses the same port for HTTP and WebSocket, so a verified Telnet entry also confirms the HTTP/WebSocket entry on the same address.
- Scans are disabled while a session is active; the connected machine is not scanned again.
- A controller error (`error:N`, `ALARM:`) is reported to the UI and does not close the session. Only transport failures and timeouts close the connection, and reconnection is manual.
- Set `CNC_UI_FLUIDNC_WS_TRACE` to enable opt-in tracing of the WebSocket and discovery traffic.

Tuning parameters (in milliseconds for timeouts): `websocket_rx_buffer_bytes`, `websocket_max_line_len`, `websocket_ack_timeout_ms`, `websocket_status_timeout_ms`, `websocket_connect_timeout_ms`. See `config/fluidnc-test.yaml` for an example profile.

Known limitations: discovery scans only ports 80 and 23, which are hardcoded; devices reconfigured to other ports are not found. A status timeout closes the session, and automatic reconnection is not implemented yet.

## Emulator configuration

The emulator loads axis travel limits, maximum velocities, accelerations, and spindle limits from `config/emulator.yaml`.

Configuration units:

- Axis travel: millimeters
- Axis velocity and feed rate: millimeters per minute
- Axis acceleration: millimeters per second squared
- Spindle speed: revolutions per minute

Axis ranges are machine-coordinate soft limits. Review the configuration and work-coordinate offsets before running a test program.

## G-code support

The emulator implements a limited G-code subset for UI development. It supports rapid and linear moves (`G0`, `G1`), XY-plane arcs (`G2`, `G3` with `I`/`J` center offsets), absolute and incremental positioning (`G90`, `G91`), work-coordinate selection (`G54`–`G59.3`), feed rate, spindle commands (`M3`, `M4`, `M5`, and `S`), dwell (`G4`), coolant commands (`M7`–`M9`), and basic program control.

The GRBL adapter communicates with GRBL 1.1 over serial and supports status and settings queries, jogging, coordinate-system commands, spindle control, and basic program commands. Program lines are sent sequentially; this is not a buffered streaming implementation.

Neither backend is a complete LinuxCNC interpreter. Unsupported commands or machine-specific behavior may produce errors or differ from a production controller. Check programs and machine behavior in a safe environment before use.

## Coordinate systems

On the emulator's first launch, it creates `config/coordinate_systems.yaml`
with zero offsets for `G54`–`G59.3`. The emulator saves subsequent changes
there, so the offsets persist across emulator restarts.

These values are specific to the machine/setup and are ignored by Git to avoid
committing personal work-zero settings. `config/coordinate_systems.example.yaml`
shows the file format with zero-valued example offsets.

The UI and emulator expose the LinuxCNC-style work-coordinate systems `G54`–`G59.3`. Work coordinates are distinct from machine coordinates; machine-coordinate travel limits are checked independently.

## Safety and limitations

- The emulator is software only and does not control a physical machine.
- The emulator socket API is project-specific, is not NML, and is not compatible with arbitrary LinuxCNC clients.
- The LinuxCNC, `linux_cnc`, and `fluid_nc` backends are debug placeholders and do not control real machines.
- The FluidNC WebSocket adapter has been verified only on a bench setup with a read-only status and settings cycle. Jog, homing, spindle, and program streaming must be validated on the target machine before use.
- GRBL serial commands are not an emergency stop. Use the machine's hardware E-stop.
- The web interface has no authentication. Do not expose it to untrusted networks or the public internet.
- This application is not a safety controller. Emulator and adapter behavior is intended for development and testing, not machine commissioning or safety validation.
- Before operating a physical machine, independently validate the hardware, configuration, toolpaths, and machine behavior.

## License

This project is licensed under the GNU General Public License v3.0 or, at your option, any later version. See [`LICENSE`](LICENSE) for the full text.
