#!/usr/bin/env python3
"""Capture the README screenshots of the CNC UI web app (task: characteristic
screenshots). Requires the backend pair running:

    cargo run --bin cnc-emulator
    cargo run --bin cnc-web

Usage:  python3 scripts/take_screenshots.py [base-url]

The shots are produced by driving the real UI (clicking the actual buttons,
waiting for status polling to fill telemetry), so every image matches the
current build. Written against Playwright's sync API with the bundled
headless Chromium; `--no-sandbox` allows running as root in containers.
"""

import sys
from pathlib import Path

from playwright.sync_api import sync_playwright

BASE_URL = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:8080"
OUT_DIR = Path(__file__).resolve().parent.parent / "docs" / "images"
VIEWPORT = {"width": 1440, "height": 900}


def shot(page, name: str) -> None:
    """Writes one full-page-state screenshot into docs/images/<name>."""
    page.screenshot(path=str(OUT_DIR / name))
    print("captured", name)


def wait_connected(page) -> None:
    """Waits until the status poller reports the emulator connection."""
    page.wait_for_function(
        "document.querySelector('#connection').textContent.trim() !== '' && "
        "!document.querySelector('.connection-chip')?.classList.contains('connection-offline')",
        timeout=15000,
    )


def main() -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as bridge:
        browser = bridge.chromium.launch(args=["--no-sandbox"])
        page = browser.new_page(viewport=VIEWPORT, device_scale_factor=1)
        # The UI polls /api/status continuously, so `networkidle` never fires;
        # load the DOM and then wait for the connection chip explicitly.
        page.goto(BASE_URL, wait_until="domcontentloaded")
        wait_connected(page)
        # Let telemetry, toolpath load and jog dial render settle.
        page.wait_for_timeout(2500)

        # 1. Main dashboard: position panel + telemetry + JOG dial + 3D viewer.
        shot(page, "main-dashboard.png")

        # 2. Machines dialog (grouped interfaces list).
        page.click("#connection-machines-button")
        page.wait_for_timeout(2500)  # background scan fills the list
        shot(page, "machines.png")
        page.click("#machines-dialog-close")

        # 3. Macro editor dialog opened on a seeded Rosetta example macro.
        # The right sidebar defaults to the FILES tab; switch to MACROS first,
        # then click the first card (evaluate-bypass keeps this robust against
        # actionability quirks of custom elements).
        page.click("#macros-tab")
        page.wait_for_timeout(400)
        page.evaluate(
            "document.querySelector('#macro-list .macro-row-open')?.click()"
        )
        page.wait_for_timeout(700)
        shot(page, "macro-editor.png")
        page.evaluate("document.querySelector('#macro-dialog').close()")

        # 4. Program editor with the auto-loaded test.gcode.
        page.evaluate("document.querySelector('#open-program-editor').click()")
        page.wait_for_timeout(700)
        shot(page, "program-editor.png")
        page.evaluate("document.querySelector('#close-program-editor').click()")

        # 5. Message log dialog (filters + calendar + rows).
        page.evaluate("document.querySelector('#message-log-dialog').showModal()")
        page.wait_for_timeout(800)
        shot(page, "message-log.png")
        page.evaluate("document.querySelector('#message-log-dialog').close()")

        browser.close()
    print("done ->", OUT_DIR)
    return 0


if __name__ == "__main__":
    sys.exit(main())
