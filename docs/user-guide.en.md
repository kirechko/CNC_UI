# CNC UI — User Guide

CNC UI is a control panel for CNC machines with two interchangeable frontends:
a desktop application (Tauri) and the same interface served by a built-in web
server to any browser on your network. It talks to FluidNC controllers over the
network, GRBL 1.1 boards over serial COM ports, and ships with a full software
emulator so you can learn the interface safely, without hardware.

> **Safety first.** This application is not a safety controller. Before running
> a physical machine, independently verify the wiring, limits switches, feeds,
> and the program itself in a dry-run environment. Serial commands are not an
> emergency stop — always keep access to the machine's hardware E-stop.

## 1. Getting started

### 1.1 Desktop application

Start the emulator first (unless you connect to real hardware):

```bash
cargo run --bin cnc-emulator          # reads config/emulator.yaml
cargo run --bin linuxcnc-ui           # the desktop window
```

The desktop app connects to the emulator automatically through a local socket
(`/tmp/linuxcnc-ui-emulator.sock`, override with `CNC_EMULATOR_SOCKET`).

### 1.2 Web interface

```bash
cargo run --bin cnc-web               # serves on http://0.0.0.0:8080
```

Open `http://<host>:8080` in a browser. Options (environment variables):

| Variable | Meaning | Default |
|---|---|---|
| `CNC_WEB_ADDRESS` | bind address:port | `0.0.0.0:8080` |
| `CNC_UI_AUTH_USER` + `CNC_UI_AUTH_PASSWORD` | enable HTTP Basic auth | disabled |
| `CNC_UI_AUTH=on` / `off` | force auth on (random password is logged) / explicitly off | auto |
| `CNC_UI_LOG_DB` | SQLite database location | `~/.local/state/linuxcnc-ui/messages.sqlite3` |
| `CNC_EMULATOR_SOCKET` | emulator socket path | `/tmp/linuxcnc-ui-emulator.sock` |

Authorization protects the whole web UI with one username/password pair; the
browser shows a login dialog on the first request. Leave the credentials unset
for trusted-lab use, set them on any shared network.

## 2. Choosing a machine

The header chip with the connection state opens the **Machines dialog**. The
list always contains:

* the local **emulator** (shown only while the emulator process is running);
* **GRBL** boards found on serial ports (`ttyUSB*`, `ttyACM*`, `ttyS*`, `COMn`);
* **FluidNC** controllers found on the network (WebSocket :80 and Telnet :23
  probes confirmed by the `$I` identity reply), and FluidNC boards plugged in
  over USB (recognized from the boot banner; when the banner reports a Wi-Fi
  address, the machine appears under its network identity).

Machines with several reachable interfaces are grouped: a small header names
the physical machine and its transports hang below as indented variants —
click the variant you want to talk through. Clicking any row switches the
backend live; the loaded program and toolpath are reloaded on the new machine.
The refresh button re-runs the whole scan; results persist between restarts.

GRBL probes toggle DTR/RTS and reset Arduino boards — while a machine is
connected, its own port is never re-probed. Baud rate is 115200 by default
(`CNC_GRBL_BAUD=9600` for legacy boards).

## 3. POSITION panel

* **WPOS / MPOS** buttons toggle the work and machine coordinate columns; the
  buttons stay aligned above their columns at any panel width.
* The **G54…G59.3** select picks the active work-coordinate system.
* Each axis row shows its value, a **set-zero** and a **home** button. On a
  5- or 6-axis machine every configured axis gets its own row (the panel
  scrolls when the rows do not fit the allotted height).
* **Zero all** writes the current position as the work zero of the selected
  system; **Home all** starts homing.
* The **telemetry strip** below the buttons shows live motion data: current
  feed with the active modal mode (`Jog`, `G0`, `G1`, `G2`, `G3`, `Homing`),
  spindle RPM, and **coolant lamps** (Mist M7 / Flood M8 glow while on).
* Clicking a work-coordinate value opens a dialog to type an exact position.

## 4. JOG panel

Two control styles, switched by the header buttons: **dial** and **keypad**.
Two motion modes: **incremental** (press = one step) and **continuous**
(hold = keep moving until release/stop).

### 4.1 Dial

The circular dial covers eight directions; the rings encode four step sizes.
Click a sector to step, or hold and sweep in continuous mode. Rotary axes
**A/B/C** appear as columns to the right of Z when the machine reports them;
their header shows the unit (`°` for angular axes, `mm` for linear ones).
Stepped buttons carry fixed distances; the Z and rotary columns share the
incremental/continuous switch.

### 4.2 Keypad

X/Y/Z buttons with diagonals, Z±, and — when configured — A±, B±, C± columns
plus a step select per rotary axis (the same axes the dial shows).

### 4.3 Feed rates

Between the dial and the spindle section sits the **per-axis feed row**:
XY, Z and each rotary axis has its own jog speed. The button opens a preset
list (generated from the axis maximum velocity the same way the spindle presets
are built) plus an **edit** line for an arbitrary value. Values are stored
locally and pushed to the controller; the legacy dialog button (sliders icon)
still edits XY/Z and the shared rotary speed.

The **Stop** square button (dial center and keypad) cancels all jog motion.

## 5. Spindle

The RPM slider, a preset button (six evenly spaced values between the
configured minimum and maximum, plus custom entry), and clockwise /
counter-clockwise start buttons. Clicking an active direction stops the
spindle. The panel mirrors the commanded and actual RPM from telemetry.

## 6. Program and 3D viewer

* **Load** a G-code file from the Files panel or the viewer toolbar; the
  program is validated, sent to the controller and drawn as a toolpath.
* The viewer legend explains the line colors: gray dashed = rapid G0,
  solid = feed G1, arcs = G2/G3. The already-traversed part of the path dims
  as the program runs (backend-reported progress — GRBL, FluidNC and emulator
  all report it in the work coordinate frame).
* **Run / Pause / Resume / Abort** mirror the controller state; the footer
  shows the current line, the remaining-time estimate and the latest messages.
* The **Editor** dialog (pencil button) edits the loaded program in place and
  re-loads it on Save; the orientation cube picks standard views, drag/zoom
  pan the scene, and the projection toggle switches perspective/orthographic.
* **Stock** loads an STL/OBJ mesh around the workpiece for context.

## 7. FILES panel

Three sources (when connected to FluidNC): **host files** (jailed to
`CNC_UI_FILES_ROOT`), **controller flash**, and **SD card**. The panel shows
free/used space of the displayed file system, file sizes in the list, and —
when an SD card is missing or not mounted — its status in place of the file
list. Navigation, upload/download, mkdir/rename/move/copy/delete behave the
same for all three sources.

## 8. MACROS panel

Create, edit, delete named G-code macros with per-controller invocation
metadata (e.g. `M650` on LinuxCNC-style backends, a named event on FluidNC).
The database ships **pre-seeded Rosetta example macros** so the panel is not
empty on the first start:

| Macro | Purpose |
|---|---|
| Hole array | generative rectangular hole pattern with parameters |
| Z zero (simple probe) | touch-probe work zero with `G38.2` + `G10 L20` |
| 3D probe align (blank) | measure the blank corner/edge and align the toolpath (Transform rotate+translate) |
| Bake G41/G42/G40 (GRBL) | cutter-compensation bake launcher (Transform phase D) |
| Peck drill (simple moves) | G83/G73 substitution target for grbl/fluidnc |

The canonical sources of these examples are the Macro-IR YAML files under
`config/macros/`; substitution presets live in `config/rosetta.yaml` and in the
`rosetta_settings` table (G83/G73 → peck-drill macro on grbl and fluidnc).
Execution of macros through the Rosetta engine is planned; today the panel
manages definitions and metadata.

## 9. Message log

The footer line shows the latest alarm/message; the panel (list icon) opens
the full history with severity/source filters, a date-range calendar, search,
sorting and pagination, plus "copy visible" / "save as text". Entries persist
in the SQLite database across restarts.

## 10. Settings

The gear button opens interface settings: **theme** (dark, light, green, gray)
and **language** (English, Русский, 中文, فارسی). Layout choices — column and
panel heights — are remembered per browser.

## 11. Keyboard and accessibility

While the JOG panel has focus: arrow keys drive X/Y, PageUp/PageDown drive Z,
hold `Ctrl` for continuous mode; every control carries a styled tooltip and an
accessible label in the active language.

## 12. Troubleshooting

* **"GET response: missing header terminator"** while browsing FluidNC files —
  fixed in current builds (the parser now accepts LF-only HTTP responses);
  update the app if you still see it.
* **Machine not discovered** — confirm the controller is on the same /24
  network or add it manually via repeated refresh; Telnet-only devices are
  confirmed over WebSocket after `$ESP120`.
* **Toolpath not highlighted during a GRBL run** — check the work offset:
  progress is computed in the work frame of the active coordinate system.
* **Web UI asks for login** — someone started `cnc-web` with
  `CNC_UI_AUTH*` variables; unset them (or `CNC_UI_AUTH=off`) for open use.

Full API and architecture details: the project README and `docs/`.
