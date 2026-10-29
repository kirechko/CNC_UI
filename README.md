# CNC UI

A CNC control application built with Rust and Tauri. It provides a desktop interface and an optional browser-based web interface, plus a standalone software emulator for development and testing without a physical machine.

The UI communicates with the selected backend through a project-specific CNC API (`cnc_core`). The emulator uses a local Unix-domain socket and a JSON protocol inspired by LinuxCNC; it is **not NML-compatible** and is not a replacement for a LinuxCNC controller.

## Features

- Tauri desktop application and optional browser-based web interface (the same UI)
- Machine discovery and live switching: FluidNC controllers on the LAN (WebSocket/Telnet probes) and GRBL 1.1 boards on serial COM ports, all listed in one machines dialog
- XY/Z/rotary jog controls with dial and key-pad modes, incremental and continuous movement, per-axis configurable speeds
- G-code program upload, toolpath visualization in a 3D scene (stock, projection toggle, fit), and run/pause/resume/abort controls with line tracking
- File manager: browse the host filesystem and the FluidNC flash/SD storage, open G-code/stock/text files, upload, download, rename, move, copy, delete
- User macros: add/edit/delete named macro definitions with per-controller invocation metadata (e.g. an `M6xx` mapping for LinuxCNC-style backends or a custom command for FluidNC); stored persistently in SQLite
- Position readouts, work-coordinate selection (`G54`–`G59.3`), zeroing and homing controls
- Spindle speed and direction controls with telemetry display
- Machine console with command history and a persistent message log (severity/source/date filters, pagination)
- Localized interface (English, Russian, Chinese, Persian) and multiple color themes
- Standalone emulator with simulated motion, spindle, coolant, homing, and soft limits
- YAML configuration for emulator axis limits and dynamics

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

To run the browser-based web interface instead, start the emulator and run either the desktop binary in web mode:

```bash
cargo run --bin linuxcnc-ui -- --web
```

or the standalone web backend binary `cnc-web`, which serves the same UI without linking the Tauri desktop runtime:

```bash
RUST_LOG=info cargo run --bin cnc-web
```

Logs are written to stderr through `tracing`; `RUST_LOG` controls the verbosity (default `info`).

The web server listens on `0.0.0.0:8080`; override it with `CNC_WEB_ADDRESS` (for example `CNC_WEB_ADDRESS=127.0.0.1:8021`) and open `http://localhost:8080` in a browser. The server has no authentication, so use it only on a trusted network.

To use a different emulator socket path for the UI, set `CNC_EMULATOR_SOCKET`:

```bash
CNC_EMULATOR_SOCKET=/tmp/my-cnc.sock cargo run --bin linuxcnc-ui
```

The emulator accepts an optional socket path followed by an optional configuration-file path:

```bash
cargo run --bin cnc-emulator -- /tmp/my-cnc.sock config/emulator.yaml
```

## Persistent storage

The message log, command history, cached discovery endpoints and user macros live in a single SQLite database:

```text
$XDG_STATE_HOME/linuxcnc-ui/messages.sqlite3
```

or `~/.local/state/linuxcnc-ui/messages.sqlite3` by default. Set `CNC_UI_LOG_DB` to override the location. The schema is created automatically on startup.

## Architecture

The desktop and web interfaces share the browser-based UI in `web/` (plain ES modules). The Tauri application embeds this interface and exposes backend commands through `invoke`; the web mode serves the same files through Axum and mirrors every command as a JSON API endpoint (`/api/...`). The frontend helper `invokeOrFetch` picks the transport automatically.

UI commands are handled by `CNCController`, which queues commands on a worker thread and maintains a cached status snapshot. The worker talks to adapters implementing the shared `CncApi` trait from the `cnc_core` crate:

- `EmAdapter` — the local emulator over a Unix-domain socket and a JSON protocol;
- FluidNC WebSocket adapter — status/jog/spindle/program over the controller's WebSocket;
- GRBL 1.1 adapter — serial line-based protocol with realtime commands and stop-and-wait program streaming;
- debug placeholder adapters for backends that do not control real machines.

## Backend selection

There is no adapter configuration file. The backend always starts with the local emulator adapter (socket `/tmp/linuxcnc-ui-emulator.sock` by default, overridable through `CNC_EMULATOR_SOCKET`); the adapter connects lazily, so the UI boots cleanly even when the emulator process is absent and recovers as soon as it appears.

All discovered machines — the local emulator, FluidNC controllers found on the network, and GRBL boards detected on serial ports — are listed in the machines dialog and are selected (switched between) by clicking a row. Discovery can be re-run at any time; the result cache persists in the SQLite database.

## FluidNC over WebSocket

The FluidNC WebSocket adapter works as follows:

- Discovery scans the IPv4 networks of local interfaces (no wider than `/24`) plus previously cached endpoints. Each candidate is probed on WebSocket (TCP 80) and Telnet (TCP 23). A device is accepted only after a read-only `$I` response confirms FluidNC.
- Clicking a machine in the machines dialog connects to it; the selection can be changed at any time.
- After connecting, the adapter reads `$ESP130`, `$ESP131`, `$ESP120`, and `$ESP121`. When `$ESP120=ON`, the WebSocket endpoint is added to the machine table on the port reported by `$ESP121`. FluidNC uses the same port for HTTP and WebSocket, so a verified Telnet entry also confirms the HTTP/WebSocket entry on the same address.
- Scans are disabled while a session is active; the connected machine is not scanned again.
- A controller error (`error:N`, `ALARM:`) is reported to the UI and does not close the session. Only transport failures and timeouts close the connection, and reconnection is manual.
- Set `CNC_UI_FLUIDNC_WS_TRACE` to enable opt-in tracing of the WebSocket and discovery traffic.

Timeouts, buffer sizes and line limits use the built-in adapter defaults; there is no per-backend configuration file.

Known limitations: discovery scans only ports 80 and 23, which are hardcoded; devices reconfigured to other ports are not found. A status timeout closes the session, and automatic reconnection is not implemented yet.

## GRBL over serial

The GRBL adapter discovers boards by probing candidate serial ports (filtered by common USB-serial adapters), sending a soft reset and reading the firmware banner; only GRBL 1.x is accepted. Connected machines appear in the same machines dialog and can be selected at any time.

The adapter supports status (`?`) and settings (`$$`) queries, homing, coordinate-system commands, spindle control, incremental and continuous jogging, and program streaming with stop-and-wait pacing. Manual commands are refused while a program streams, and programs larger than the adapter limit are rejected.

Known limitations: jog and spindle telemetry depend on the controller reporting them; unsupported M-codes (e.g. coolant variants) are skipped with a warning. GRBL serial commands are not an emergency stop — use the machine's hardware E-stop.

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

The GRBL adapter communicates with GRBL 1.1 over serial and supports status and settings queries, jogging, coordinate-system commands, spindle control, and basic program commands. Program lines are sent sequentially with stop-and-wait flow control; consult the GRBL documentation for the supported command subset.

Neither backend is a complete LinuxCNC interpreter. Unsupported commands or machine-specific behavior may produce errors or differ from a production controller. Check programs and machine behavior in a safe environment before use.

## Coordinate systems

On the emulator's first launch, it creates `config/coordinate_systems.yaml` with zero offsets for `G54`–`G59.3`. The emulator saves subsequent changes there, so the offsets persist across emulator restarts.

These values are specific to the machine/setup and are ignored by Git to avoid committing personal work-zero settings. `config/coordinate_systems.example.yaml` shows the file format with zero-valued example offsets.

The UI and emulator expose the LinuxCNC-style work-coordinate systems `G54`–`G59.3`. Work coordinates are distinct from machine coordinates; machine-coordinate travel limits are checked independently.

## Macros

The MACROS tab in the right sidebar manages user-defined macros. Each macro has a name, description, G-code body, and a list of per-controller invocation mappings (controller kind, mechanism, command), for example an `M6xx` code for a LinuxCNC-style backend or a named event for FluidNC. The mappings are metadata: the UI does not execute them against a controller yet.

The whole macro list is validated by the backend (size limits, unique ids, unique controller per macro) and stored atomically in the shared SQLite database. In desktop mode macros are saved through Tauri commands; in the browser they go through `GET/PUT /api/macros`.

## Safety and limitations

- The emulator is software only and does not control a physical machine.
- The emulator socket API is project-specific, is not NML, and is not compatible with arbitrary LinuxCNC clients.
- The LinuxCNC, `linux_cnc`, and `fluid_nc` placeholder adapters are debug stubs and do not control real machines.
- The FluidNC WebSocket adapter has been verified only on a bench setup with a read-only status and settings cycle. Jog, homing, spindle, and program streaming must be validated on the target machine before use.
- GRBL serial commands are not an emergency stop. Use the machine's hardware E-stop.
- Macro invocation mappings are metadata only; assigning an M-code to a macro does not register it with a controller.
- The web interface has no authentication. Do not expose it to untrusted networks or the public internet.
- This application is not a safety controller. Emulator and adapter behavior is intended for development and testing, not machine commissioning or safety validation.
- Before operating a physical machine, independently validate the hardware, configuration, toolpaths, and machine behavior.

## License

This project is licensed under the GNU General Public License v3.0 or, at your option, any later version. See [`LICENSE`](LICENSE) for the full text.
