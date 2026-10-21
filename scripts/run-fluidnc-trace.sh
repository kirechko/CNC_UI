#!/usr/bin/env bash
set -euo pipefail

log_file="${CNC_UI_TRACE_LOG:-/tmp/fluidnc-trace.log}"
printf 'Лог FluidNC будет записываться в %s\n' "$log_file"

exec env \
  CNC_UI_FLUIDNC_WS_TRACE=1 \
  CNC_UI_HTTP_STATUS_TRACE=1 \
  CNC_UI_CONFIG=config/fluidnc-test.yaml \
  cargo run -- --web >"$log_file" 2>&1
