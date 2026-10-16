#!/usr/bin/env bash
# Screenshot the app inside a private headless Weston (never the user's session).
#
#   scripts/shot.sh <out.png> [settle-seconds] [binary]
#
# Extra app env (HERMES_DESKTOP_*, SLINT_BACKEND, …) passes through.
set -euo pipefail
cd "$(dirname "$0")/.."
out="$(realpath -m "$1")"
settle="${2:-3}"
bin="${3:-target/release/hermes-desktop-rs}"
if [[ -z "${_HERMES_SHOT_INNER:-}" ]]; then
  HEADLESS_DEBUG=1 _HERMES_SHOT_INNER=1 exec scripts/headless.sh "$0" "$out" "$settle" "$bin"
fi
work="$(mktemp -d "${TMPDIR:-/tmp}/hermes-shot.XXXXXX")"
trap 'kill $app 2>/dev/null || true; rm -rf "$work"' EXIT
RUST_LOG="${RUST_LOG:-warn}" "$bin" >/dev/null 2>"$work/app.log" &
app=$!
sleep "$settle"
(cd "$work" && weston-screenshooter >/dev/null 2>&1)
shot="$(ls -t "$work"/wayland-screenshot-*.png 2>/dev/null | head -1)"
[[ -n "$shot" ]] || { echo "no screenshot produced" >&2; tail -5 "$work/app.log" >&2; exit 4; }
mv "$shot" "$out"
echo "$out"
