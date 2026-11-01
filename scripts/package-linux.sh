#!/usr/bin/env bash
set -euo pipefail

# Build and package the unified Linux executable with web, desktop, and emulator modes.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$ROOT/dist"
STAGE="$DIST/linux-x86_64"
ARCHIVE="$DIST/cnc-ui-linux-x86_64.tar.gz"

cd "$ROOT"
# Build the unified executable with Tauri desktop support; install the documented Linux prerequisites first.
cargo build --release --bin cnc_ui
rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/config" "$STAGE/web" "$STAGE/data"
install -m 755 target/release/cnc_ui "$STAGE/bin/"
cp -a config/emulator.yaml "$STAGE/config/"
cp -a config/rosetta.yaml "$STAGE/config/"
cp -a config/macros "$STAGE/config/"
cp -a test.gcode "$STAGE/"
cp -a web/. "$STAGE/web/"
cp -a README.md LICENSE "$STAGE/"
cat > "$STAGE/run.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT"
mkdir -p data
export CNC_WEB_ADDRESS="${CNC_WEB_ADDRESS:-127.0.0.1:8080}"
export CNC_EMULATOR_ADDRESS="${CNC_EMULATOR_ADDRESS:-127.0.0.1:8765}"
export CNC_UI_AUTH="${CNC_UI_AUTH:-off}"
printf 'Starting unified CNC UI in web mode with emulator.\n'
printf 'Open http://%s; press Ctrl+C to stop both services.\n' "$CNC_WEB_ADDRESS"
exec ./bin/cnc_ui --web --emu
EOF
chmod +x "$STAGE/run.sh"
cat > "$STAGE/README-RUN.txt" <<'EOF'
CNC UI unified Linux distribution

Run ./run.sh to start the browser UI with its bundled emulator, then open
http://127.0.0.1:8080. Press Ctrl+C to stop the application and emulator.

The same executable supports desktop mode: ./bin/cnc_ui --desktop [--emu].
Linux desktop mode requires compatible system WebKitGTK/Tauri runtime dependencies.
The archive also contains web assets and editable configuration beside the executable.
Runtime data is stored under ./data and ./config.

The demo launcher disables HTTP authentication. Do not expose this control
interface to untrusted networks. Set CNC_WEB_ADDRESS before running to change
its bind address.
EOF
rm -f "$ARCHIVE"
tar -C "$DIST" -czf "$ARCHIVE" linux-x86_64
printf 'Created %s\n' "$ARCHIVE"
