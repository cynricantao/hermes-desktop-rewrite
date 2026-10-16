#!/usr/bin/env python3
"""Seed one long conversation into the PRIVATE live-test backend's session store.

    HERMES_HOME=$TMPDIR/hermes-live-home scripts/seed-long-session.py [messages] [label]

Writes `messages` (default 600) alternating user/assistant turns (markdown lists, code
blocks, wrapped paragraphs) as a synthetic Claude-Code-format JSONL, then imports it with
`hermes sessions import --from claude <file>` under that HERMES_HOME. No model call is made.
Refuses to run against the real ~/.hermes. Prints the importer's output (the new session id).
"""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

home = os.environ.get("HERMES_HOME", "")
real = Path.home() / ".hermes"
if not home or Path(home).resolve() == real.resolve():
    sys.exit("refusing: set HERMES_HOME to the private live-test home (not ~/.hermes)")
if not (Path(home) / "config.yaml").exists():
    sys.exit(f"refusing: {home} has no config.yaml; run scripts/live-test-backend.sh first")

n = int(sys.argv[1]) if len(sys.argv) > 1 else 600
# Optional second argument: a label prefixed to every user turn, so several small fixtures
# can be told apart in the sidebar (tests/live_backend.rs `sessions` section).
label = f"{sys.argv[2]} " if len(sys.argv) > 2 else ""
lines: list[dict] = [{"type": "summary", "summary": f"long-transcript-fixture ({n} messages)"}]
for i in range(n // 2):
    lines.append({"type": "user", "cwd": "/tmp", "sessionId": "fixture",
                  "message": {"role": "user", "content": f"{label}Question {i}: explain item {i} briefly."}})
    body = (
        f"**Item {i}.** This is a *seeded* answer with a list:\n\n"
        f"- first point about {i}\n- second point\n- third point\n\n"
        f"```python\ndef item_{i}(x):\n    return x * {i}\n```\n\n"
        f"And a closing paragraph that is long enough to wrap across a couple of lines in "
        f"the transcript column so row heights vary like a real conversation does ({i})."
    )
    lines.append({"type": "assistant", "cwd": "/tmp", "sessionId": "fixture",
                  "message": {"role": "assistant", "content": [{"type": "text", "text": body}]}})

with tempfile.NamedTemporaryFile("w", suffix=".jsonl", dir=home, delete=False) as f:
    f.write("\n".join(json.dumps(x) for x in lines) + "\n")
    path = f.name
try:
    r = subprocess.run(["hermes", "sessions", "import", "--from", "claude", path],
                       env={**os.environ, "HERMES_HOME": home}, text=True, capture_output=True)
    print(r.stdout.strip() or r.stderr.strip())
    sys.exit(r.returncode)
finally:
    os.unlink(path)
