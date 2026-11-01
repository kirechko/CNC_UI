"""Validated listener, INI, program-storage, and HAL settings for the CNC_UI bridge."""

from __future__ import annotations

import hashlib
import hmac
import ipaddress
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


@dataclass(frozen=True)
class LaserPins:
    """Explicit HAL signal names for an optional laser output."""

    enable: Optional[str] = None
    power_command: Optional[str] = None
    power_feedback: Optional[str] = None
    mode: Optional[str] = None


@dataclass(frozen=True)
class BridgeConfig:
    """Immutable runtime configuration shared by listener, protocol, and NML layers."""

    ini_path: Path
    bind: str
    port: int
    token: Optional[str]
    program_directory: Path
    heartbeat_timeout: float = 1.5
    poll_interval: float = 0.05
    laser_pins: LaserPins = LaserPins()

    @classmethod
    def from_ini(cls, ini_path: Path, bind: str, port: int, token: Optional[str]) -> "BridgeConfig":
        """Resolve INI/storage paths and require a token for non-loopback listeners."""
        resolved = ini_path.expanduser().resolve(strict=True)
        if not 1 <= port <= 65535:
            raise ValueError("bridge port must be in 1..65535")
        address = ipaddress.ip_address(bind)
        if not address.is_loopback and not token:
            raise ValueError("non-loopback bind requires LCNC_ADAPTER_TOKEN")
        directory = Path(os.environ.get("LCNC_PROGRAM_DIR", resolved.parent)).resolve()
        return cls(resolved, bind, port, token, directory)

    @property
    def machine_id(self) -> str:
        """Create a stable identifier from the INI basename and canonical path."""
        digest = hashlib.sha256(str(self.ini_path).encode("utf-8")).hexdigest()[:12]
        return f"linuxcnc:{self.ini_path.stem.lower()}:{digest}"

    def token_valid(self, provided: Optional[str]) -> bool:
        """Compare an optional shared secret in constant time."""
        return self.token is None or (provided is not None and hmac.compare_digest(self.token, provided))
