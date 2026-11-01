"""Async newline-delimited JSON server with one controller owner and observers."""

from __future__ import annotations

import asyncio
import json
import logging
import time
from dataclasses import dataclass
from typing import Any, Optional

from config import BridgeConfig
from interface import LinuxCNCInterface
from protocol import failure, parse_packet, success


@dataclass
class Client:
    """A connected bridge peer with an assigned role and heartbeat timestamp."""
    reader: asyncio.StreamReader
    writer: asyncio.StreamWriter
    role: str
    last_heartbeat: float
    name: str


class BridgeServer:
    """Serve bounded JSON commands while enforcing controller and laser fail-safe rules."""

    def __init__(self, interface: LinuxCNCInterface, config: BridgeConfig) -> None:
        self.interface = interface
        self.config = config
        self.server: Optional[asyncio.AbstractServer] = None
        # Keep exactly one control owner; registered peers begin as read-only observers.
        self.clients: set[int] = set()
        self.control_client: Optional[int] = None
        self.client_objects: dict[int, Client] = {}
        self._stopping = asyncio.Event()
        self._last_full_status = 0.0

    async def start(self) -> None:
        """Bind the configured address; BridgeConfig prevents unauthenticated remote binds."""
        self.server = await asyncio.start_server(self._handle_client, self.config.bind, self.config.port)

    async def close(self) -> None:
        """Close listeners, demote clients, and force configured laser output off."""
        self._stopping.set()
        if self.server is not None:
            self.server.close()
            await self.server.wait_closed()
        # Do not await close() on peers before closing their writers; otherwise shutdown can hang.
        for client in tuple(self.client_objects.values()):
            client.writer.close()
        await asyncio.gather(*(client.writer.wait_closed() for client in self.client_objects.values()), return_exceptions=True)
        await asyncio.to_thread(self.interface._laser_off)

    async def _handle_client(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        """Process one client's request stream until disconnect or protocol failure."""
        client_id = id(writer)
        # New sessions cannot command the machine until a validated connect obtains control.
        client = Client(reader, writer, "observer", time.monotonic(), "unknown")
        self.client_objects[client_id] = client
        try:
            while not self._stopping.is_set():
                # The frame limit is checked before JSON parsing to bound untrusted input.
                raw = await reader.readline()
                if not raw:
                    break
                if len(raw) > 1024 * 1024:
                    # Oversized lines are rejected before parsing and terminate the peer session.
                    await self._write_error(writer, 0, "request_too_large", "request exceeds 1 MiB")
                    break
                # Do not carry an earlier packet's id into errors for malformed JSON.
                packet: dict[str, Any] = {}
                try:
                    packet = parse_packet(raw)
                    response = await self._dispatch(client_id, client, packet)
                except Exception as error:
                    logging.exception("bridge request failed")
                    packet_id = packet.get("id", 0)
                    await self._write_error(writer, packet_id, self._error_code(error), str(error))
                else:
                    await self._write(writer, response)
        finally:
            self._release_client(client_id)
            writer.close()
            try:
                await writer.wait_closed()
            except OSError:
                pass

    async def _dispatch(self, client_id: int, client: Client, packet: dict[str, Any]) -> dict[str, Any]:
        """Validate envelope, role, token, and heartbeat before dispatching to NML in a thread."""
        request_id = int(packet["id"])
        command = str(packet.get("command", ""))
        payload = packet.get("payload", {})
        if not isinstance(payload, dict):
            raise ValueError("payload must be an object")
        if command == "set_laser_mode" and client.role == "control":
            if self._heartbeat_expired(client):
                self._release_client(client_id)
                raise TimeoutError("control heartbeat expired; reconnect before changing laser mode")
            result = await asyncio.to_thread(self.interface.execute, command, payload)
            return success(request_id, result)
        if command == "connect":
            client.name = str(payload.get("client_name", "CNC_UI"))[:128]
            token = payload.get("token")
            if not self.config.token_valid(token):
                raise PermissionError("invalid bridge token")
            if self.control_client is None:
                self.control_client = client_id
                client.role = "control"
            result = await asyncio.to_thread(self.interface.execute, command, payload)
            result["role"] = client.role
            result["machine_id"] = self.config.machine_id
            return success(request_id, result)
        # Observers may watch status but must never perform NML control writes.
        if command == "connect":
            # Permit a reconnect after ownership was released; the v1 hello still returns capabilities.
            client.last_heartbeat = time.monotonic()
            result = await asyncio.to_thread(self.interface.execute, "connect", payload)
            result["role"] = client.role
            result["machine_id"] = self.config.machine_id
            return success(request_id, result)
        if client.role == "control" and command == "heartbeat":
            client.last_heartbeat = time.monotonic()
        elif command == "heartbeat":
            return success(request_id, {"alive": True, "role": client.role})
        elif command not in {"status"} and client.role != "control":
            raise PermissionError("observer clients cannot issue control commands")
        if command not in {"status", "heartbeat", "connect"}:
            if client.role != "control":
                raise PermissionError("connect and obtain the control role before sending commands")
            if self._heartbeat_expired(client):
                self._release_client(client_id)
                raise TimeoutError("control heartbeat expired; reconnect before commanding motion")
        result = await asyncio.to_thread(self.interface.execute, command, payload)
        return success(request_id, result)

    def _heartbeat_expired(self, client: Client) -> bool:
        """On missed heartbeats, feed-hold and laser-off before revoking control."""
        expired = time.monotonic() - client.last_heartbeat > self.config.heartbeat_timeout
        if expired:
            try:
                import linuxcnc
                self.interface.command.feed_hold()
            except Exception:
                logging.exception("feed hold failed after heartbeat loss")
            self.interface._laser_off()
        return expired

    def _release_client(self, client_id: int) -> None:
        """Release control ownership on disconnect and force laser output safe."""
        self.clients.discard(client_id)
        self.client_objects.pop(client_id, None)
        if self.control_client == client_id:
            self.control_client = None
            self.interface._laser_off()


    async def _write_error(self, writer: asyncio.StreamWriter, request_id: int, code: str, message: str) -> None:
        """Write one protocol error and drain the socket."""
        await self._write(writer, failure(request_id, code, message))

    async def _write(self, writer: asyncio.StreamWriter, packet: dict[str, Any]) -> None:
        """Serialize one response line with a bounded backpressure wait."""
        writer.write((json.dumps(packet, separators=(",", ":")) + "\n").encode("utf-8"))
        await asyncio.wait_for(writer.drain(), timeout=2.0)

    def _error_code(self, error: Exception) -> str:
        """Map expected Python failures into stable client-visible error codes."""
        if isinstance(error, PermissionError):
            return "permission_denied"
        if isinstance(error, TimeoutError):
            return "timeout"
        if isinstance(error, (ValueError, KeyError, TypeError)):
            return "bad_request"
        return "linuxcnc_error"
