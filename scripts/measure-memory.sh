#!/usr/bin/env bash
# Start the app, wait for its first frame (+ settle time), then report which GL driver
# libraries are mapped and the process memory (RSS / PSS breakdown).
#
#   scripts/measure-memory.sh [binary]
#
# Runs inside a private headless Weston by default (never touches your session).
# On your real GPU session (run it yourself):  HERMES_MEASURE_LIVE=1 scripts/measure-memory.sh
set -euo pipefail
cd "$(dirname "$0")/.."
bin="${1:-target/release/hermes-desktop-rs}"
if [[ -z "${HERMES_MEASURE_LIVE:-}" && -z "${_HERMES_MEASURE_INNER:-}" ]]; then
  echo "# display: headless weston, renderer=${HEADLESS_RENDERER:-pixman}"
  _HERMES_MEASURE_INNER=1 exec scripts/headless.sh "$0" "$bin"
fi
log="$(mktemp "${TMPDIR:-/tmp}/hermes-mem.XXXXXX")"
RUST_LOG=startup=info,warn NO_COLOR=1 "$bin" 2>"$log" >/dev/null &
p=$!
trap 'kill $p 2>/dev/null || true; rm -f "$log"' EXIT
for _ in $(seq 100); do grep -q 'phase="first frame"' "$log" 2>/dev/null && break; sleep 0.1; done
sleep "${SETTLE:-3}"
grep -o 'phase="first frame" elapsed_ms=[0-9.]*' "$log" || echo "(no first frame seen)"
echo "GL/driver libraries mapped:"
grep -oE '/usr/lib[^ ]*/lib(nvidia-[a-z0-9-]+|gallium[^ /]*|EGL_[a-z]+|LLVM[^ /]*|vulkan|GLX_[a-z]+)[^ ]*' "/proc/$p/maps" | sort -u | sed 's/^/  /' || true
echo "memory:"
grep -E '^(Rss|Pss|Pss_Anon|Pss_File|Private_Dirty)' "/proc/$p/smaps_rollup" | sed 's/^/  /'
