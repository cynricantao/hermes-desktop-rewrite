#!/usr/bin/env bash
# Long-conversation cost with the real binary and renderer (docs/PERF.md).
#
#   HERMES_DESKTOP_PORT=<private port> scripts/measure-long-transcript.sh <stored-session-id> [binary]
#
# Opens the session (seed one with scripts/seed-long-session.py), waits, then sweeps the
# transcript to the top and back once (HERMES_DESKTOP_AUTOSCROLL). Prints CPU (% of one
# core) and RSS for three windows: idle after load, during the sweep, idle after the sweep.
# Headless by default; only ever point HERMES_DESKTOP_PORT at scripts/live-test-backend.sh.
set -euo pipefail
cd "$(dirname "$0")/.."
sid="${1:?usage: measure-long-transcript.sh <stored-session-id> [binary]}"
bin="${2:-target/release/hermes-desktop-rs}"
[[ -n "${HERMES_DESKTOP_PORT:-}" ]] || { echo "set HERMES_DESKTOP_PORT to the private test backend" >&2; exit 2; }
if [[ -z "${HERMES_MEASURE_LIVE:-}" && -z "${_HERMES_MEASURE_INNER:-}" ]]; then
  echo "# display: headless weston, scale=${HEADLESS_SCALE:-1}, renderer=${HERMES_DESKTOP_RENDERER:-software}"
  _HERMES_MEASURE_INNER=1 exec scripts/headless.sh "$0" "$sid" "$bin"
fi
log="$(mktemp "${TMPDIR:-/tmp}/hermes-long.XXXXXX")"
HERMES_DESKTOP_OPEN_SESSION="$sid" HERMES_DESKTOP_AUTOSCROLL="${SWEEP_AT:-8}" RUST_LOG=warn "$bin" >/dev/null 2>"$log" &
p=$!
trap 'kill $p 2>/dev/null || true; rm -f "$log"' EXIT
hz=$(getconf CLK_TCK)
ticks() { awk '{print $14+$15}' "/proc/$p/stat"; }
rss() { awk '/^VmRSS/{printf "%.1f", $2/1024}' "/proc/$p/status"; }
window() { # label seconds
  local t0 s0 t1 s1; t0=$(ticks); s0=$(date +%s.%N); sleep "$2"; t1=$(ticks); s1=$(date +%s.%N)
  awk -v l="$1" -v a="$t0" -v b="$t1" -v hz="$hz" -v s0="$s0" -v s1="$s1" -v r="$(rss)" \
    'BEGIN{printf "%-22s cpu %5.1f%% of one core over %.1fs (%d ticks)  RSS %s MiB\n", l, 100*(b-a)/hz/(s1-s0), s1-s0, b-a, r}'
}
sleep 3                                  # startup + session load
window "idle, session loaded" 4          # ends at ~7 s; sweep starts at SWEEP_AT (8 s)
sleep 1
window "sweep top and back" "${SWEEP_S:-12}"
sleep 2
window "idle after sweep" 5
