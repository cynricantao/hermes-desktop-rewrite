#!/usr/bin/env bash
# Run a command against a private headless Weston (pixman software renderer).
# The user's session is never touched: DISPLAY is unset and WAYLAND_DISPLAY points at a
# socket owned by this script, which is torn down on exit.
#
#   scripts/headless.sh <command> [args...]
#
# Env: HEADLESS_W / HEADLESS_H (default 1440x900), HEADLESS_SOCKET (default random),
#      HEADLESS_RENDERER=pixman (software, default) or gl (compositor on the GPU via EGL).
set -euo pipefail
sock="${HEADLESS_SOCKET:-hermes-headless-$$}"
rt="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
weston --backend=headless --renderer="${HEADLESS_RENDERER:-pixman}" --shell=kiosk \
  --width="${HEADLESS_W:-1440}" --height="${HEADLESS_H:-900}" \
  --socket="$sock" --idle-time=0 ${HEADLESS_DEBUG:+--debug} >/dev/null 2>&1 &
wp=$!
trap 'kill $wp 2>/dev/null || true' EXIT
for _ in $(seq 50); do [[ -S "$rt/$sock" ]] && break; sleep 0.1; done
[[ -S "$rt/$sock" ]] || { echo "headless weston did not start" >&2; exit 3; }
unset DISPLAY
export WAYLAND_DISPLAY="$sock"
"$@"
