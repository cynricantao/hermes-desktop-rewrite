#!/usr/bin/env bash
# Start a PRIVATE Hermes backend for tests/live_backend.rs, isolated from your everyday one.
#
#   scripts/live-test-backend.sh            # prints HERMES_DESKTOP_PORT=<port>, runs until killed
#
# It uses its own HERMES_HOME (default ~/.cache/hermes-desktop-rs/live-home) holding:
#   * a copy of ~/.hermes/config.yaml with approvals.mode=manual (so the approval card
#     fires) and the external memory provider disabled (test chats don't go into memory);
#   * only the model-provider key from ~/.hermes/.env (no Telegram / dashboard secrets);
#   * a copy of auth.json;
#   * two throwaway named profiles (`swe` "Software Engineer", `ea` "Executive Assistant")
#     with a minimal config.yaml each, for the settings "Applies to" tests.
# Its sessions, logs and memories live in that directory, so test chats never show up in
# your real session list. `--isolated` keeps it from attaching to your running backend.
#
# The home MUST NOT live under ~/.hermes: the backend anchors the profiles root (and
# `?profile=default`) at the Hermes root, and a HERMES_HOME anywhere under ~/.hermes
# (e.g. $TMPDIR=~/.hermes/cache/scratch) resolves that root to the real ~/.hermes, so
# profile-scoped settings writes would land in your real profiles.
set -euo pipefail
src="${HERMES_HOME:-$HOME/.hermes}"
home="${HERMES_LIVE_HOME:-$HOME/.cache/hermes-desktop-rs/live-home}"
real_root="$(realpath -m "$HOME/.hermes")"
case "$(realpath -m "$home")/" in
  "$real_root"/*) echo "refusing: HERMES_LIVE_HOME ($home) is inside ~/.hermes" >&2; exit 2 ;;
esac
mkdir -p "$home"
chmod 700 "$home"
python3 - "$src/config.yaml" "$home/config.yaml" <<'PY'
import re, sys
s = open(sys.argv[1]).read()
s = re.sub(r'(?m)^(approvals:\n  mode:) .*$', r'\1 "manual"', s)
s = re.sub(r"(?m)^(memory:\n  provider:) .*$", r"\1 ''", s)
# The approval checks need an empty allowlist, and the reasoning checks a known default.
s = re.sub(r"(?m)^command_allowlist:.*\n(?:(?:[ \t]+|-).*\n)*", "", s)
s = re.sub(r"(?m)^(\s*reasoning_effort:) .*$", r"\1 medium", s)
open(sys.argv[2], "w").write(s)
PY
grep -E '^(COMMANDCODE_API_KEY|OPENCODE_GO_API_KEY)=' "$src/.env" > "$home/.env" || true
cp -p "$src/auth.json" "$home/auth.json" 2>/dev/null || true
chmod 600 "$home/config.yaml" "$home/.env"
# Throwaway named profiles, recreated on every start.
for spec in "swe:Software Engineer" "ea:Executive Assistant"; do
  name="${spec%%:*}"; title="${spec#*:}"
  dir="$home/profiles/$name"
  mkdir -p "$dir"
  printf 'model:\n  default: test-model\n' > "$dir/config.yaml"
  printf 'ui_meta:\n  hermes-bots:\n    title: %s\n' "$title" > "$dir/profile.yaml"
done
# A private HOME too: the backend writes profile alias wrappers to $HOME/.local/bin and
# other per-user files under $HOME, which must not land in the real home directory.
mkdir -p "$home/home"
echo "# private backend home: $home (approvals.mode=manual, profiles: swe, ea, HOME=$home/home)" >&2
HOME="$home/home" HERMES_HOME="$home" exec hermes serve --isolated --host 127.0.0.1 --port 0 --skip-build
