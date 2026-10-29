#!/usr/bin/env bash
# Simulation: fake controller (real daemon4 on a fake board) + fake camera + CtoP browser service, all on private
# ports/sockets and a private data dir. Never touches the real printer, camera or controller socket.
#   tools/sim_stack.sh            # prints the URL to open; Ctrl-C stops everything
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN="${CTOP_SIM_DIR:-/run/user/$(id -u)/ctop-sim}"; mkdir -p "$RUN"
export CTOP_DATA="$RUN/data"; mkdir -p "$CTOP_DATA"
SOCK="$RUN/controller.sock"; CAMPORT="${CAMPORT:-7892}"; export CTOP_WEB_PORT="${CTOP_WEB_PORT:-7896}"
BIN="${CTOP_BIN:-$ROOT/src-tauri/target/debug/ctop}"
pids=(); trap 'kill "${pids[@]}" 2>/dev/null || true' EXIT INT TERM
python3 -I "$ROOT/tools/fake_controller.py" --sock "$SOCK" --speedup "${SPEEDUP:-40}" >"$RUN/controller.log" 2>&1 & pids+=($!)
python3 -I "$ROOT/tools/fake_camera.py" --port "$CAMPORT" --paper-y 6 --ctl-sock "$SOCK" ${FAKE_CAMERA_ARGS:-} >"$RUN/camera.log" 2>&1 & pids+=($!)
[ "${NO_CAL:-0}" = 1 ] || python3 -I "$ROOT/tools/fake_camera.py" --write-cal "$CTOP_DATA/camera_cal.json" --pose 270 305 ${FAKE_CAMERA_ARGS:-} >/dev/null
echo "{\"controllerSocket\":\"$SOCK\",\"cameraUrl\":\"http://127.0.0.1:$CAMPORT\"}" >"$CTOP_DATA/ctop-settings.json"
for _ in $(seq 50); do [ -S "$SOCK" ] && break; sleep 0.2; done
CTOP_CONTROLLER_SOCK="$SOCK" "$BIN" --serve & pids+=($!)
wait
