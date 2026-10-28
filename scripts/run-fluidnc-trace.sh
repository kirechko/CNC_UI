#!/usr/bin/env bash
set -euo pipefail

log_file="${CNC_UI_TRACE_LOG:-/tmp/fluidnc-trace.log}"
printf 'Лог FluidNC будет записываться в %s\n' "$log_file"

# There is no CNC_UI_CONFIG anymore: start the web backend, open the machines
# dialog in the browser and click the FluidNC board to connect to it.
exec env \
  CNC_UI_FLUIDNC_WS_TRACE=1 \
  CNC_UI_HTTP_STATUS_TRACE=1 \
  cargo run -- --web >"$log_file" 2>&1
