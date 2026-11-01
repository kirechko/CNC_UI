# LinuxCNC bridge

`lcnc_adapter` is a companion Python process intended to run on the LinuxCNC host. The Python `linuxcnc` module and NML backend are only available there; FerroHUB talks to this bridge over TCP using newline-delimited JSON. The application package and protocol identifiers retain their legacy `CNC_UI` naming for compatibility.

## Requirements

- LinuxCNC 2.8+ with its Python modules available to the interpreter used by the service.
- Python 3.8+.
- LinuxCNC and the bridge launched with the same machine INI environment/configuration.

## Run

```sh
python3 lcnc_adapter/main.py --ini /path/to/machine.ini --bind 127.0.0.1 --port 8766
```

Use `LCNC_ADAPTER_TOKEN` (or `--token`) when binding a non-loopback address. The matching Rust client reads `LCNC_ADAPTER_TOKEN` from its environment for `connect`. Prefer an SSH tunnel for remote UI access; this protocol does not encrypt traffic. Configure `LCNC_PROGRAM_DIR` to a writable, LinuxCNC-approved G-code directory.

FerroHUB connects by selecting a manually entered endpoint encoded as `linuxcnc:host:port` in the machine-selection API. The bridge implements newline JSON envelopes `{v,id,command,payload}` and responses `{v,id,ok,payload}` or `{v,id,ok:false,error}`. The current application integration includes the handshake, status and MDI paths; the companion implements broader machine and program commands, but the Rust/UI control path remains partial and is not production-verified. The address is not advertised by network auto-discovery; add it manually.

## Machine INI additions (optional)

```ini
[CNC_UI]
LASER_ENABLE_PIN = cncui.laser-enable
LASER_POWER_PIN = cncui.laser-power
LASER_POWER_FEEDBACK_PIN = cncui.laser-power-feedback
LASER_MODE_PIN = cncui.laser-dynamic-mode
LASER_MIN_POWER = 0.0
LASER_MAX_POWER = 1.0
LASER_DYNAMIC_POWER = 1
LASER_SHUTOFF_ON_PAUSE = 1
```

HAL names are explicit; the bridge never guesses a pin or claims laser support unless enable and power pins are provided. Use a hardware interlock and physical E-stop independently of this service. The bridge is not a safety-rated controller.

## Safety and operational scope

The listener defaults to loopback. One client owns control; other clients may request status only. A control heartbeat timeout issues feed hold and best-effort laser-off, and disconnect/abort/E-stop/pause with configured shutoff also turns the laser off. HAL failure must be handled by a hardware watchdog/interlock. The bridge validates axis indexes, jog limits, MDI size, upload size and safe basename-only program paths.

The interface covers status, machine power/E-stop state, home/unhome, jog, WCS/zero, MDI/batches, uploaded program run/pause/resume/abort, spindle, coolant, optional laser, and overrides. `program_validate` currently returns a minimal accepted result; full interpreter dry-run validation and sim-config integration tests remain follow-up work.

## Tests

Pure protocol tests do not import LinuxCNC:

```sh
python3 -m unittest discover -s lcnc_adapter/tests
```

Integration testing requires a LinuxCNC sim configuration and must be performed on a host/container that supplies the LinuxCNC Python/NML bindings.
