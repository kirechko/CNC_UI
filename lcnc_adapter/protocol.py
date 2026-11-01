"""Strict JSON line protocol parsing, command schemas, and response envelopes."""

from __future__ import annotations

import json
from typing import Any

PROTOCOL_VERSION = 1
MAX_PACKET_BYTES = 1024 * 1024
COMMANDS = {
    "connect", "heartbeat", "status", "power", "estop", "estop_reset",
    "jog_start", "jog_stop", "jog_increment", "home", "unhome", "set_wcs",
    "zero_axis", "program_upload", "program_run", "program_pause", "program_resume",
    "program_abort", "program_validate", "mdi", "mdi_batch", "wait_complete",
    "set_spindle", "set_coolant", "set_laser", "set_laser_mode", "override",
}


def parse_packet(raw: bytes) -> dict[str, Any]:
    """Decode one bounded UTF-8 JSON packet and require its v1 envelope fields."""
    if len(raw) > MAX_PACKET_BYTES:
        raise ValueError("packet exceeds protocol size limit")
    packet = json.loads(raw.decode("utf-8"))
    if not isinstance(packet, dict) or packet.get("v") != PROTOCOL_VERSION:
        raise ValueError("unsupported protocol version or invalid envelope")
    if not isinstance(packet.get("id"), int) or packet["id"] < 0:
        raise ValueError("request id must be a non-negative integer")
    command = packet.get("command")
    if command not in COMMANDS:
        raise ValueError(f"unsupported bridge command: {command}")
    if not isinstance(packet.get("payload", {}), dict):
        raise ValueError("payload must be a JSON object")
    packet.setdefault("payload", {})
    return packet


def success(request_id: int, payload: dict[str, Any]) -> dict[str, Any]:
    """Build a versioned successful response with an echoed correlation id."""
    return {"v": PROTOCOL_VERSION, "id": request_id, "ok": True, "payload": payload}


def failure(request_id: int, code: str, message: str) -> dict[str, Any]:
    """Build a versioned error response without leaking Python exception objects."""
    return {"v": PROTOCOL_VERSION, "id": request_id, "ok": False, "error": {"code": code, "message": message}}
