# CNC UI

A desktop CNC control interface built with Rust and GTK4. The project includes a standalone software emulator for developing and testing the UI without connecting to a physical machine.

The UI communicates with the emulator over a local Unix-domain socket using a project-specific, LinuxCNC-inspired API. This API is intended for this application; it is **not NML-compatible** and is not a replacement for a LinuxCNC controller.

## Features

- GTK4 desktop interface with XY and Z jog controls
- Configurable XY and Z jog feed rates
- G-code file loading and toolpath visualization
- 3D orientation cube with clickable faces
- Tool visualization synchronized with emulator position and spindle telemetry
- Simulated spindle speed ramp, direction, and coolant states
- Simulated E-stop, alarm reset, homing, and work-coordinate selection
- Standalone emulator communicating with the UI over a Unix socket
- YAML configuration for axis travel, maximum velocity, acceleration, and spindle settings

## Requirements

- Linux
- Rust toolchain and Cargo
- GTK4 development libraries
- GTK and GLib system dependencies required by the `gtk4` Rust crate

Install the GTK4 development package using your distribution's package manager if it is not already installed.

## Build

Run these commands from the repository root:

```bash
cargo check --all-targets
cargo build --bin cnc-emulator
cargo build --bin linuxcnc-ui
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

The emulator must be running before the UI starts.

To use a different socket path, set `CNC_EMULATOR_SOCKET` when launching the UI:

```bash
CNC_EMULATOR_SOCKET=/tmp/my-cnc.sock cargo run --bin linuxcnc-ui
```

The emulator accepts an optional socket path followed by an optional configuration-file path:

```bash
cargo run --bin cnc-emulator -- /tmp/my-cnc.sock config/emulator.yaml
```

## Emulator configuration

The emulator loads axis travel limits, maximum velocities, accelerations, and spindle limits from `config/emulator.yaml`.

Configuration units:

- Axis travel: millimeters
- Axis velocity and feed rate: millimeters per minute
- Axis acceleration: millimeters per second squared
- Spindle speed: revolutions per minute

Axis ranges are machine-coordinate soft limits. Review the configuration and work-coordinate offsets before running a test program.

## G-code support

The emulator implements a limited G-code subset for UI development. Supported behavior includes common rapid and linear moves, XY-plane circular moves, absolute and incremental positioning, feed rate, spindle commands, dwell, coolant commands, and program control.

This is **not a complete LinuxCNC interpreter**. Unsupported commands may stop program execution with an error. Check that a program uses only supported commands before running it in the emulator.

## Coordinate systems

The UI and emulator expose the LinuxCNC-style work-coordinate systems `G54`–`G59.3`. Work coordinates are distinct from machine coordinates; machine-coordinate travel limits are checked independently.

## Safety and limitations

- The emulator is software only and does not control a physical machine.
- The project-specific socket API is not NML and is not compatible with arbitrary LinuxCNC clients.
- Emulator behavior is intended for UI development and testing, not machine commissioning or safety validation.
- Do not use this emulator or an unverified UI build as a safety controller for a real machine.
- A future real-machine backend will require a separate LinuxCNC adapter and validation against the actual machine configuration.

## License

This project is licensed under the GNU General Public License v3.0 or, at your option, any later version. See [`LICENSE`](LICENSE) for the full text.
