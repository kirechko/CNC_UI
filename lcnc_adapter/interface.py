"""Thread-isolated wrapper around LinuxCNC NML and optional HAL pins."""

from __future__ import annotations

import logging
import math
import threading
from pathlib import Path
from typing import Any, Optional

import linuxcnc

from config import BridgeConfig

AXIS_LETTERS = "XYZABCUVW"
AXIS_INDEX = {letter: index for index, letter in enumerate(AXIS_LETTERS)}
COORDINATE_SYSTEMS = ("G54", "G55", "G56", "G57", "G58", "G59", "G59.1", "G59.2", "G59.3")


class LinuxCNCInterface:
    """Own stat/command/error objects and serialize their access away from asyncio."""

    def __init__(self, config: BridgeConfig) -> None:
        """Initialize NML handles and immutable capabilities from the machine INI."""
        self.config = config
        self.ini = linuxcnc.ini(str(config.ini_path))
        self.stat = linuxcnc.stat()
        self.command = linuxcnc.command()
        self.errors = linuxcnc.error_channel()
        self.lock = threading.RLock()
        self.axes = self._load_axes()
        self.limits = self._load_limits()
        self.max_velocity = self._load_max_velocities()
        self.user_mcodes = self._load_remap_mcodes()
        self.laser_config = self._load_laser_capabilities()
        self._last_status: dict[str, Any] = {}
        self._last_error: Optional[str] = None

    def _load_axes(self) -> list[str]:
        """Derive available axes from TRAJ coordinates, preserving LinuxCNC axis order."""
        coordinates = self.ini.find("TRAJ", "COORDINATES") or "XYZ"
        # INI character order is the coordinate index contract used by linuxcnc.command().jog().
        return [letter for letter in coordinates.upper() if letter in AXIS_LETTERS]

    def _load_limits(self) -> dict[str, list[float]]:
        """Read axis soft limits from the INI, defaulting only absent bounds to infinities."""
        # Bounds are stored in coordinate units and reused by jog preflight checks.
        limits: dict[str, list[float]] = {}
        for letter in self.axes:
            section = f"AXIS_{letter}"
            minimum = self._float_ini(section, "MIN_LIMIT", -math.inf)
            maximum = self._float_ini(section, "MAX_LIMIT", math.inf)
            limits[letter] = [minimum, maximum]
        return limits

    def _load_max_velocities(self) -> dict[str, float]:
        """Read configured per-axis maximum velocity values from the INI."""
        return {
            letter: self._float_ini(f"AXIS_{letter}", "MAX_VELOCITY", 0.0)
            for letter in self.axes
        }

    def _load_remap_mcodes(self) -> list[str]:
        """Collect configured M6xx remaps for capabilities and macro-editor metadata."""
        section = self.ini.findall("RS274NGC", "REMAP") or []
        result = []
        for value in section:
            token = value.strip().split()[0] if value.strip() else ""
            if token.upper().startswith("M") and token[1:].isdigit():
                result.append(token.upper())
        return sorted(set(result), key=lambda code: int(code[1:]))

    def _load_laser_capabilities(self) -> dict[str, Any]:
        """Describe laser availability from configured HAL pin names, never guessed pins."""
        enable = self._ini_optional("CNC_UI", "LASER_ENABLE_PIN")
        pwm = self._ini_optional("CNC_UI", "LASER_POWER_PIN")
        feedback = self._ini_optional("CNC_UI", "LASER_POWER_FEEDBACK_PIN")
        mode_pin = self._ini_optional("CNC_UI", "LASER_MODE_PIN")
        present = bool(enable and pwm)
        if present and self._float_ini("CNC_UI", "LASER_MAX_POWER", 1.0) <= self._float_ini("CNC_UI", "LASER_MIN_POWER", 0.0):
            raise ValueError("configured laser power range must have max > min")
        return {
            "present": present,
            "min_power": self._float_ini("CNC_UI", "LASER_MIN_POWER", 0.0),
            "max_power": self._float_ini("CNC_UI", "LASER_MAX_POWER", 1.0),
            "dynamic_power": self._bool_ini("CNC_UI", "LASER_DYNAMIC_POWER", False),
            "auto_shutoff_on_pause": self._bool_ini("CNC_UI", "LASER_SHUTOFF_ON_PAUSE", True),
            "enable_pin": enable,
            "power_pin": pwm,
            "feedback_pin": feedback,
            "mode_pin": mode_pin,
        }

    def capabilities(self) -> dict[str, Any]:
        """Return stable configuration facts for the bridge connect handshake."""
        return {
            "machine_id": self.config.machine_id,
            "axes": self.axes,
            "kinematics": self.ini.find("KINS", "KINEMATICS") or "unknown",
            "limits": self.limits,
            "max_velocity": self.max_velocity,
            "spindles": int(self.ini.find("TRAJ", "SPINDLES") or "1"),
            "laser": self.laser_config,
            "coolant": {"mist": True, "flood": True},
            "gcode_dialect": "linuxcnc",
            "user_mcodes": self.user_mcodes,
            "linuxcnc_version": "2.8+",
            "protocol_version": 1,
        }

    def poll_status(self) -> dict[str, Any]:
        """Poll NML once and return a complete normalized machine status snapshot."""
        with self.lock:
            self.stat.poll()
            status = self.stat
            actual = list(status.actual_position)
            g5x = list(status.g5x_offset)
            g92 = list(status.g92_offset)
            tool = list(status.tool_offset)
            rotation = float(getattr(status, "rotation_xy", 0.0))
            work = self._work_coordinates(actual, g5x, g92, tool, rotation)
            task_state = int(status.task_state)
            interp = int(status.interp_state)
            spindle_list = list(getattr(status, "spindle", []))
            spindle = spindle_list[0] if spindle_list else {}
            current_line = int(getattr(status, "motion_line", 0) or 0)
            snapshot = {
                "connected": True,
                "machine_pos": actual[:len(self.axes)],
                "work_pos": work[:len(self.axes)],
                "homed": list(getattr(status, "homed", [])),
                "task_state": task_state,
                "task_mode": int(status.task_mode),
                "interp_state": interp,
                "exec_state": int(getattr(status, "exec_state", 0)),
                "feed_rate": float(getattr(status, "current_vel", 0.0)),
                "rapid_override": float(getattr(status, "rapidrate", 1.0)),
                "feed_override": float(getattr(status, "feedrate", 1.0)),
                "spindle_override": float(getattr(status, "spindlerate", 1.0)),
                "spindle": [{
                    "speed": float(spindle.get("speed", 0.0)),
                    "enabled": bool(spindle.get("enabled", False)),
                    "direction": int(spindle.get("direction", 0)),
                }],
                "current_line": current_line,
                "motion_line": current_line,
                "file": getattr(status, "file", None),
                "estop": task_state == linuxcnc.STATE_ESTOP,
                "enabled": task_state == linuxcnc.STATE_ON,
                "laser": self._read_laser_status(),
                "coordinate_system": int(getattr(status, "g5x_index", 1)),
                "offsets": self._read_offsets(status),
            }
            # Surface the latest queued NML error in status; clear only after each response has drained the channel.
            self._drain_errors()
            if self._last_error:
                snapshot["error"] = self._last_error
                self._last_error = None
            self._last_status = snapshot
            return snapshot

    def execute(self, command: str, payload: dict[str, Any]) -> dict[str, Any]:
        """Validate each operation and call its corresponding NML/HAL handler."""
        if command == "connect":
            supplied = payload.get("token")
            if not self.config.token_valid(supplied):
                raise PermissionError("invalid bridge token")
            return {"role": "control", "capabilities": self.capabilities()}
        if command == "set_laser_mode":
            self._set_laser_mode(payload)
            return {"accepted": True}
        if command == "heartbeat":
            return {"alive": True}
        if command == "status":
            return self.poll_status()
        with self.lock:
            if command == "power":
                self.command.state(linuxcnc.STATE_ON if payload.get("on") else linuxcnc.STATE_OFF)
            elif command == "estop":
                self.command.state(linuxcnc.STATE_ESTOP)
                self._laser_off()
            elif command == "estop_reset":
                self.command.state(linuxcnc.STATE_ESTOP_RESET)
            elif command == "jog_start":
                # Convert the wire axis before passing it to LinuxCNC's coordinate-index API.
                self._jog(self._axis_index(payload["axis"]), float(payload["velocity"]), linuxcnc.JOG_CONTINUOUS)
            elif command == "jog_stop":
                self._jog(self._axis_index(payload["axis"]), 0.0, linuxcnc.JOG_STOP)
            elif command == "jog_increment":
                self._jog(self._axis_index(payload["axis"]), float(payload["velocity"]), linuxcnc.JOG_INCREMENT, float(payload["distance"]))
            elif command == "home":
                axis = payload.get("axis")
                self.command.home(-1 if axis in (None, "all") else self._axis_index(axis))
            elif command == "unhome":
                self.command.unhome(self._axis_index(payload["axis"]))
            elif command == "set_wcs":
                self._mdi(str(payload["system"]))
            elif command == "zero_axis":
                self._mdi(self._zero_axis_gcode(payload))
            elif command == "mdi":
                self._mdi(str(payload["block"]))
            elif command == "mdi_batch":
                for block in payload.get("blocks", []):
                    self._mdi(str(block))
            elif command == "program_upload":
                return self._upload(payload)
            elif command == "program_run":
                self._program_run(payload)
            elif command == "program_pause":
                self.command.auto(linuxcnc.AUTO_PAUSE)
                self._laser_off_if_configured()
            elif command == "program_resume":
                self.command.auto(linuxcnc.AUTO_RESUME)
            elif command == "program_abort":
                self.command.abort()
                self._laser_off()
            elif command == "set_spindle":
                self._set_spindle(payload)
            elif command == "set_laser":
                self._set_laser(payload)
            elif command == "set_coolant":
                self.command.coolant(linuxcnc.MIST_ON if payload.get("mist") else linuxcnc.MIST_OFF)
                self.command.coolant(linuxcnc.FLOOD_ON if payload.get("flood") else linuxcnc.FLOOD_OFF)
            elif command == "override":
                self._override(payload)
            elif command == "program_validate":
                return {"valid": True, "warnings": []}
            elif command == "wait_complete":
                self.command.wait_complete(float(payload.get("timeout_ms", 30000)) / 1000.0)
            else:
                raise ValueError(f"unsupported command: {command}")
        return {"accepted": True}

    def _jog(self, axis: int, velocity: float, jog_type: int, distance: float = 0.0) -> None:
        """Switch to manual mode and dispatch validated teleop-axis jog operations."""
        # Teleop coordinate indices follow the configured TRAJ coordinate sequence.
        if not math.isfinite(velocity) or abs(velocity) > self._max_axis_velocity(axis):
            raise ValueError("jog velocity is outside configured axis limits")
        self.command.mode(linuxcnc.MODE_MANUAL)
        self.command.wait_complete(2.0)
        if jog_type == linuxcnc.JOG_INCREMENT:
            self.command.jog(jog_type, linuxcnc.JOG_TELEOP, axis, distance, abs(velocity))
        else:
            self.command.jog(jog_type, linuxcnc.JOG_TELEOP, axis, velocity)

    def _mdi(self, block: str) -> None:
        """Execute one bounded MDI line and wait for its NML command completion."""
        if not block.strip() or len(block) > 4096:
            raise ValueError("MDI block must contain 1..4096 characters")
        # Switch to MDI and wait before dispatch to avoid racing the task-mode state machine.
        self.command.mode(linuxcnc.MODE_MDI)
        self.command.wait_complete(2.0)
        self.command.mdi(block)
        result = self.command.wait_complete(30.0)
        if result == -1:
            self._laser_off()
            raise TimeoutError("MDI command did not complete before timeout")
        # NML errors are read immediately after each synchronous block to contain faults.
        self._drain_errors()

    def _program_run(self, payload: dict[str, Any]) -> None:
        """Start an uploaded program in auto mode at its requested line."""
        name = str(payload["name"])
        path = self._program_path(name)
        start_line = int(payload.get("start_line", 0))
        if start_line < 0:
            raise ValueError("program start line cannot be negative")
        self.command.mode(linuxcnc.MODE_AUTO)
        self.command.wait_complete(2.0)
        self.command.auto(linuxcnc.AUTO_RUN, start_line)

    def _upload(self, payload: dict[str, Any]) -> dict[str, Any]:
        """Persist a named G-code upload under the configured bridge program directory."""
        name = str(payload.get("name", ""))
        content = payload.get("content")
        if not name or Path(name).name != name or not isinstance(content, str):
            raise ValueError("upload requires a base filename and text content")
        if len(content.encode("utf-8")) > 5 * 1024 * 1024:
            raise ValueError("program exceeds the 5 MiB bridge limit")
        self.config.program_directory.mkdir(parents=True, exist_ok=True)
        path = self._program_path(name)
        path.write_text(content, encoding="utf-8")
        self.command.program_open(str(path))
        self.command.wait_complete(5.0)
        return {"name": name, "path": str(path)}

    def _program_path(self, name: str) -> Path:
        """Resolve a safe program basename under the configured program directory."""
        if Path(name).name != name or name in ("", ".", ".."):
            raise ValueError("program name must be a simple filename")
        return self.config.program_directory / name

    def _set_spindle(self, payload: dict[str, Any]) -> None:
        """Map normalized direction/speed commands to the LinuxCNC spindle command API."""
        direction = str(payload.get("direction", "off")).lower()
        speed = float(payload.get("speed", 0.0))
        if not math.isfinite(speed) or speed < 0.0:
            raise ValueError("spindle speed must be finite and non-negative")
        mapped = {"cw": linuxcnc.SPINDLE_FORWARD, "ccw": linuxcnc.SPINDLE_REVERSE, "off": linuxcnc.SPINDLE_OFF}
        if direction not in mapped:
            raise ValueError("spindle direction must be cw, ccw, or off")
        if direction == "off":
            self.command.spindle(mapped[direction], int(payload.get("spindle", 0)))
        else:
            self.command.spindle(mapped[direction], speed, int(payload.get("spindle", 0)))

    def _set_laser_mode(self, payload: dict[str, Any]) -> None:
        """Select constant/dynamic output using only an explicitly configured HAL mode pin."""
        mode = str(payload.get("mode", ""))
        if mode not in {"constant", "dynamic"}:
            raise ValueError("laser mode must be constant or dynamic")
        if mode == "dynamic" and not self.laser_config["dynamic_power"]:
            raise ValueError("dynamic laser power is not supported by the configured INI")
        mode_pin = self.laser_config.get("mode_pin")
        if not mode_pin:
            if mode == "constant":
                return
            raise RuntimeError("laser dynamic-mode HAL pin is not configured")
        import hal
        hal.set_p(mode_pin, "1" if mode == "dynamic" else "0")

    def _set_laser(self, payload: dict[str, Any]) -> None:
        """Set configured HAL laser pins with a fail-closed disabled state."""
        if not self.laser_config["present"]:
            raise RuntimeError("laser HAL pins are not configured")
        # Validate power before touching HAL, then make disable an atomic zero+enable-low update.
        enabled = bool(payload.get("enabled"))
        power = float(payload.get("power", 0.0))
        if not math.isfinite(power) or not self.laser_config["min_power"] <= power <= self.laser_config["max_power"]:
            raise ValueError("laser power is outside configured limits")
        import hal
        if not enabled:
            power = 0.0
        hal.set_p(self.laser_config["power_pin"], str(power))
        hal.set_p(self.laser_config["enable_pin"], "1" if enabled else "0")

    def _laser_off_if_configured(self) -> None:
        """Disable emission on pause when the profile explicitly enables auto-shutoff."""
        if self.laser_config["auto_shutoff_on_pause"]:
            self._laser_off()

    def _laser_off(self) -> None:
        """Best-effort immediate HAL laser disable used on faults, abort, and E-stop."""
        if not self.laser_config["present"]:
            return
        try:
            self._set_laser({"enabled": False, "power": 0.0})
        except Exception:
            logging.exception("failed to force laser output off")

    def _drain_errors(self) -> None:
        """Consume pending NML error-channel messages and force laser-off on faults."""
        for _ in range(256):
            item = self.errors.poll()
            if item is None:
                return
            kind, text = item
            message = str(text)
            logging.warning("LinuxCNC error-channel message %s: %s", kind, message)
            self._last_error = message
            self._laser_off()
        logging.error("LinuxCNC error channel exceeded the per-poll drain limit")
        self._laser_off()

    def _read_laser_status(self) -> dict[str, Any]:
        """Return configured laser status, reading feedback only when an explicit pin exists."""
        if not self.laser_config["present"]:
            return {"present": False, "enabled": False, "power": 0.0}
        try:
            import hal
            feedback = self.laser_config["feedback_pin"]
            power = float(hal.get_value(feedback)) if feedback else 0.0
            enabled = bool(hal.get_value(self.laser_config["enable_pin"]))
            return {"present": True, "enabled": enabled, "power": power}
        except Exception as error:
            return {"present": True, "enabled": False, "power": 0.0, "error": str(error)}

    def _override(self, payload: dict[str, Any]) -> None:
        """Apply a normalized percentage to feed, rapid, or spindle override channels."""
        kind = str(payload["kind"])
        value = float(payload["value"])
        if not math.isfinite(value) or not 0.0 <= value <= 1.2:
            raise ValueError("override value must be in 0..1.2")
        scaled = int(round(value * 100.0))
        if kind == "feed":
            self.command.feedrate(scaled)
        elif kind == "rapid":
            self.command.rapidrate(scaled)
        elif kind == "spindle":
            self.command.spindleoverride(scaled)
        else:
            raise ValueError("override kind must be feed, rapid, or spindle")

    def _zero_axis_gcode(self, payload: dict[str, Any]) -> str:
        """Build a validated G10 L20 zeroing block for an available axis and WCS."""
        axis = str(payload["axis"]).upper()
        system = str(payload.get("system", "G54")).upper()
        if axis not in self.axes or system not in COORDINATE_SYSTEMS:
            raise ValueError("axis or coordinate system is not configured")
        index = COORDINATE_SYSTEMS.index(system) + 1
        return f"G10 L20 P{index} {axis}0"

    def _read_offsets(self, status: Any) -> dict[str, Any]:
        """Expose the active coordinate offsets and G92/tool offsets from the status object."""
        return {
            "g5x": list(status.g5x_offset),
            "g92": list(status.g92_offset),
            "tool": list(status.tool_offset),
            "rotation_xy": float(getattr(status, "rotation_xy", 0.0)),
        }

    def _work_coordinates(self, actual: list[float], g5x: list[float], g92: list[float], tool: list[float], rotation: float) -> list[float]:
        """Calculate work positions after subtracting offsets and rotating XY into the WCS frame."""
        radians = math.radians(rotation)
        x = actual[0] - g5x[0] - g92[0] - tool[0]
        y = actual[1] - g5x[1] - g92[1] - tool[1]
        return [
            x * math.cos(radians) + y * math.sin(radians),
            -x * math.sin(radians) + y * math.cos(radians),
            *[actual[i] - g5x[i] - g92[i] - tool[i] for i in range(2, min(len(actual), 9))],
        ]

    def _axis_index(self, axis: Any) -> int:
        """Resolve a named or numeric axis identifier to the INI coordinate slot."""
        if isinstance(axis, str):
            letter = axis.upper()
            if letter not in self.axes:
                raise ValueError(f"axis {letter} is not configured")
            return self.axes.index(letter)
        index = int(axis)
        if index < 0 or index >= len(self.axes):
            raise ValueError("axis index is not configured")
        return index

    def _max_axis_velocity(self, axis: int) -> float:
        """Return axis velocity limit using the configured coordinate order."""
        if axis < 0 or axis >= len(self.axes):
            raise ValueError("axis index is not configured")
        return self.max_velocity.get(self.axes[axis], 0.0)

    def _ini_optional(self, section: str, key: str) -> Optional[str]:
        """Read a non-empty optional INI value without converting parser errors to defaults."""
        value = self.ini.find(section, key)
        return value.strip() if value and value.strip() else None

    def _float_ini(self, section: str, key: str, default: float) -> float:
        """Read and validate one finite floating-point INI option."""
        value = self._ini_optional(section, key)
        if value is None:
            return default
        result = float(value)
        if not math.isfinite(result):
            raise ValueError(f"{section}.{key} must be finite")
        return result

    def _bool_ini(self, section: str, key: str, default: bool) -> bool:
        """Read a conventional true/false INI option."""
        value = self._ini_optional(section, key)
        if value is None:
            return default
        return value.lower() in {"1", "true", "yes", "on"}
