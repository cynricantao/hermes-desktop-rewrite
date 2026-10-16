#!/usr/bin/env bash
# CPU cost of the UI while a real reply streams in (renderer comparison, docs/PERF.md).
#
#   scripts/measure-stream-cpu.sh [seconds] [binary]
#
# Sends one prompt to the real backend through HERMES_DESKTOP_AUTOSEND (creates a chat in
# your session list) and samples the app's utime+stime for N seconds. Prints CPU as % of
# one core, plus peak RSS. Headless by default; HERMES_MEASURE_LIVE=1 for your session.
# HEADLESS_SCALE=2 runs the hidden display at 2x (HiDPI).
set -euo pipefail
cd "$(dirname "$0")/.."
secs="${1:-25}"
bin="${2:-target/release/hermes-desktop-rs}"
if [[ -z "${HERMES_MEASURE_LIVE:-}" && -z "${_HERMES_MEASURE_INNER:-}" ]]; then
  echo "# display: headless weston, scale=${HEADLESS_SCALE:-1}, renderer=${HERMES_DESKTOP_RENDERER:-software}"
  _HERMES_MEASURE_INNER=1 exec scripts/headless.sh "$0" "$secs" "$bin"
fi
prompt="${STREAM_PROMPT:-Write about 600 words on the history of the printing press, as plain paragraphs with a short markdown list in the middle. No tools.}"
log="$(mktemp "${TMPDIR:-/tmp}/hermes-cpu.XXXXXX")"
HERMES_DESKTOP_AUTOSEND="$prompt" RUST_LOG=warn "$bin" >/dev/null 2>"$log" &
p=$!
trap 'kill $p 2>/dev/null || true; rm -f "$log"' EXIT
hz=$(getconf CLK_TCK)
ticks() { awk '{print $14+$15}' "/proc/$p/stat"; }
sleep 1.5                                   # past startup
t0=$(ticks); s0=$(date +%s.%N); peak=0
for _ in $(seq $((secs * 4))); do
  sleep 0.25
  r=$(awk '/^VmRSS/{print $2}' "/proc/$p/status"); (( r > peak )) && peak=$r
done
t1=$(ticks); s1=$(date +%s.%N)
awk -v a="$t0" -v b="$t1" -v hz="$hz" -v s0="$s0" -v s1="$s1" -v pk="$peak" \
  'BEGIN{printf "cpu %.1f%% of one core over %.1fs (%d ticks)  peak RSS %.1f MiB\n", 100*(b-a)/hz/(s1-s0), s1-s0, b-a, pk/1024}'
