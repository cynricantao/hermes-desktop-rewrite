#!/usr/bin/env bash
# Capture every screen of the ORIGINAL Hermes Desktop (read-only use of its built dist/) for
# docs/parity/screens/. Runs inside a private headless Weston with the e2e-style sandbox:
# own HOME/HERMES_HOME/userData, isolated backend, mock inference server. Never touches the
# user's display, backend, config or sessions, and writes nothing into the original install.
#
#   scripts/parity-capture/run.sh [only-substring]
#
# Env: HERMES_AGENT_ROOT (default ~/.hermes/hermes-agent), PARITY_OUT (default docs/parity/screens)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
agent="${HERMES_AGENT_ROOT:-$HOME/.hermes/hermes-agent}"
work="${TMPDIR:-/tmp}/parity-capture-run"
mkdir -p "$work"
# tsx + @playwright/test resolve from the agent checkout's node_modules via a scratch symlink.
ln -sfn "$agent/node_modules" "$work/node_modules"
cp "$here/capture.ts" "$work/capture.ts"
echo '{"type":"module","private":true}' > "$work/package.json"
cd "$work"
PARITY_SRC_DIR="$here" PARITY_ONLY="${1:-}" PARITY_OUT="${PARITY_OUT:-$repo/docs/parity/screens}" \
  "$repo/scripts/headless.sh" ./node_modules/.bin/tsx capture.ts
