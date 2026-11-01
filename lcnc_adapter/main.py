#!/usr/bin/env python3
"""Run the local LinuxCNC NML-to-TCP JSON bridge service."""

from __future__ import annotations

import argparse
import asyncio
import logging
import os
import signal
from pathlib import Path

from config import BridgeConfig
from interface import LinuxCNCInterface
from server import BridgeServer


def parse_args() -> argparse.Namespace:
    """Parse command-line arguments, falling back to the LinuxCNC INI environment."""
    parser = argparse.ArgumentParser(description="CNC_UI LinuxCNC TCP bridge")
    parser.add_argument("--ini", default=os.environ.get("INI_FILE_NAME"), help="LinuxCNC INI path")
    parser.add_argument("--bind", default="127.0.0.1", help="TCP bind address")
    parser.add_argument("--port", type=int, default=8766, help="TCP bind port")
    parser.add_argument("--token", default=os.environ.get("LCNC_ADAPTER_TOKEN"), help="optional client token")
    return parser.parse_args()


async def run() -> None:
    """Start the bridge server and close it cleanly on SIGINT or SIGTERM."""
    args = parse_args()
    if not args.ini:
        raise SystemExit("Specify --ini or set INI_FILE_NAME")
    config = BridgeConfig.from_ini(Path(args.ini), args.bind, args.port, args.token)
    interface = LinuxCNCInterface(config)
    server = BridgeServer(interface, config)
    await server.start()
    stopped = asyncio.Event()
    loop = asyncio.get_running_loop()
    for signum in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(signum, stopped.set)
    logging.info("LinuxCNC bridge listening on %s:%d", config.bind, config.port)
    await stopped.wait()
    await server.close()


def main() -> None:
    """Configure logging and execute the asynchronous bridge lifecycle."""
    logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
    asyncio.run(run())


if __name__ == "__main__":
    main()
