#!/usr/bin/env bash
# Measure process start → first presented frame (plus every `startup` phase mark), N runs.
#
#   scripts/measure-startup.sh [runs] [binary]
#
# Default: inside a private headless Weston (scripts/headless.sh) — the user's session is
# never touched. HEADLESS_RENDERER=pixman (default, pure software compositor) or gl (the
# hidden compositor renders through EGL on the GPU).
#
# On your real GPU session (run it yourself):
#   HERMES_MEASURE_LIVE=1 scripts/measure-startup.sh 5
# Each run opens a window, quits right after its first frame
# (HERMES_DESKTOP_EXIT_AFTER_FIRST_FRAME=1) and prints the phase table.
set -euo pipefail
cd "$(dirname "$0")/.."

runs="${1:-5}"
bin="${2:-target/release/hermes-desktop-rs}"
[[ -x "$bin" ]] || { echo "binary not found: $bin (cargo build --release -p hermes-app)" >&2; exit 2; }

if [[ -z "${HERMES_MEASURE_LIVE:-}" && -z "${_HERMES_MEASURE_INNER:-}" ]]; then
  echo "# display: headless weston, renderer=${HEADLESS_RENDERER:-pixman}"
  _HERMES_MEASURE_INNER=1 exec scripts/headless.sh "$0" "$runs" "$bin"
fi
if [[ -n "${HERMES_MEASURE_LIVE:-}" ]]; then
  echo "# display: LIVE session WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-unset}"
fi

export RUST_LOG="${RUST_LOG:-startup=info,warn}"
export HERMES_DESKTOP_EXIT_AFTER_FIRST_FRAME=1
export NO_COLOR=1

# Output: one TSV block per run — "<elapsed_ms>\t<phase>" — then the first-frame summary.
ff=()
for i in $(seq "$runs"); do
  out="$(timeout 20 "$bin" 2>&1 >/dev/null || true)"
  echo "## run $i"
  phases="$(grep ' startup: ' <<<"$out" | sed -E 's/.*phase="([^"]*)" elapsed_ms=([0-9.]+).*/\2\t\1/' || true)"
  echo "$phases"
  ff+=("$(awk -F'\t' '$2=="first frame"{print $1}' <<<"$phases")")
  sleep 0.5
done
echo "## first frame (ms): ${ff[*]}"
printf '%s\n' "${ff[@]}" | sort -n | awk '{a[NR]=$1} END{if(NR) printf "## median %.1f ms  min %.1f  max %.1f  (n=%d)\n", (NR%2?a[(NR+1)/2]:(a[NR/2]+a[NR/2+1])/2), a[1], a[NR], NR}'
