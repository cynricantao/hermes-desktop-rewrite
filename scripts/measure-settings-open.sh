#!/usr/bin/env bash
# Measure the settings panel's open time — from the panel being asked to open (Ctrl+, or
# the dev hook, i.e. the `opened` command reaching the controller) to the page's rows being
# handed to the UI thread — over N runs, in a private headless Weston.
#
#   scripts/measure-settings-open.sh [runs] [page]
#
# The number is the controller's own trace (`settings panel loaded`, target `settings`); the
# first open of a run includes the profile list, config and config-schema GETs, which is what
# a user sees when the panel first opens. Needs a backend: set HERMES_DESKTOP_PORT=<port> to
# reuse the private test backend (`scripts/live-test-backend.sh`), otherwise each run spawns
# its own isolated one (slower, and the spawn is not part of the measured interval).
set -euo pipefail
cd "$(dirname "$0")/.."
runs="${1:-10}"
page="${2:-appearance/general}"
bin="target/release/hermes-desktop-rs"
[[ -x "$bin" ]] || { echo "build first: cargo build --release -p hermes-app" >&2; exit 2; }

samples=()
for _ in $(seq "$runs"); do
  log="$(mktemp)"
  HERMES_DESKTOP_OPEN_SETTINGS="$page" RUST_LOG="warn,settings=info" \
    scripts/headless.sh "$bin" >/dev/null 2>"$log" &
  pid=$!
  ms=""
  for _ in $(seq 300); do
    ms="$(grep -m1 -o 'elapsed_ms=[0-9.]\+' "$log" 2>/dev/null | cut -d= -f2 || true)"
    [[ -n "$ms" ]] && break
    sleep 0.1
  done
  # Stop this run's own processes only: the app and weston are children of that one script.
  for child in $(pgrep -P "$pid" 2>/dev/null || true); do kill "$child" 2>/dev/null || true; done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
  [[ -n "$ms" ]] || { echo "no sample (panel never loaded); log:" >&2; tail -5 "$log" >&2; rm -f "$log"; exit 3; }
  rm -f "$log"
  samples+=("$ms")
  echo "run: ${ms} ms"
done

printf '%s\n' "${samples[@]}" | python3 -c '
import statistics, sys
v = sorted(float(x) for x in sys.stdin if x.strip())
print(f"n={len(v)} min={v[0]:.1f} median={statistics.median(v):.1f} mean={statistics.fmean(v):.1f} max={v[-1]:.1f} ms")
'
