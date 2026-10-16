#!/usr/bin/env bash
# Start a PRIVATE Hermes backend for tests/live_backend.rs, isolated from your everyday one.
#
#   scripts/live-test-backend.sh            # prints HERMES_DESKTOP_PORT=<port>, runs until killed
#
# It uses its own HERMES_HOME (default $TMPDIR/hermes-live-home) holding:
#   * a copy of ~/.hermes/config.yaml with approvals.mode=manual (so the approval card
#     fires) and the external memory provider disabled (test chats don't go into memory);
#   * only the model-provider key from ~/.hermes/.env (no Telegram / dashboard secrets);
#   * a copy of auth.json.
# Its sessions, logs and memories live in that directory, so test chats never show up in
# your real session list. `--isolated` keeps it from attaching to your running backend.
set -euo pipefail
src="${HERMES_HOME:-$HOME/.hermes}"
home="${HERMES_LIVE_HOME:-${TMPDIR:-/tmp}/hermes-live-home}"
mkdir -p "$home"
chmod 700 "$home"
python3 - "$src/config.yaml" "$home/config.yaml" <<'PY'
import re, sys
s = open(sys.argv[1]).read()
s = re.sub(r'(?m)^(approvals:\n  mode:) .*$', r'\1 "manual"', s)
s = re.sub(r"(?m)^(memory:\n  provider:) .*$", r"\1 ''", s)
open(sys.argv[2], "w").write(s)
PY
grep -E '^(COMMANDCODE_API_KEY)=' "$src/.env" > "$home/.env" || true
cp -p "$src/auth.json" "$home/auth.json" 2>/dev/null || true
chmod 600 "$home/config.yaml" "$home/.env"
echo "# private backend home: $home (approvals.mode=manual)" >&2
HERMES_HOME="$home" exec hermes serve --isolated --host 127.0.0.1 --port 0 --skip-build
