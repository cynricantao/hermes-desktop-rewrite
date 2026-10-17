#!/usr/bin/env bash
# Update Hermes Desktop (Rust) from the repository's main branch.
#
# Keeps a private clone (never your development checkout), builds a new release only when
# main moved, and swaps the binaries in atomically, keeping the previous ones as `.prev`.
# A failed build leaves the installed app untouched. Installed by scripts/install-user.sh as
# ~/.local/lib/hermes-desktop-rs/hermes-desktop-rs-update and run by the systemd user units
# in packaging/systemd/ (after a successful `hermes update`, and once a day).
#
#   hermes-desktop-rs-update                     update if main moved
#   hermes-desktop-rs-update --force             rebuild and reinstall even if unchanged
#   hermes-desktop-rs-update --after-hermes-update
#                                                only when the last `hermes update` succeeded
set -euo pipefail

REPO_URL="${HDRS_REPO_URL:-https://github.com/cynricantao/hermes-desktop-rewrite.git}"
BRANCH="${HDRS_BRANCH:-main}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}/hermes-desktop-rs"
STATE="${XDG_STATE_HOME:-$HOME/.local/state}/hermes-desktop-rs"
LIB="$HOME/.local/lib/hermes-desktop-rs"
HERMES_HOME_DIR="${HERMES_HOME:-$HOME/.hermes}"
SRC="$DATA/src"
JOBS="${HDRS_JOBS:-$(( $(nproc) > 4 ? $(nproc) / 2 : 2 ))}"
export PATH="$HOME/.cargo/bin:$PATH"

force=0
after_hermes=0
for arg in "$@"; do
  case "$arg" in
    --force) force=1 ;;
    --after-hermes-update) after_hermes=1 ;;
    -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

mkdir -p "$DATA" "$STATE" "$LIB"
exec > >(tee -a "$STATE/update.log") 2>&1
exec 9>"$STATE/update.lock"
if ! flock -n 9; then
  echo "another update is already running"
  exit 0
fi

log() { printf '%s %s\n' "$(date '+%F %T')" "$*"; }
notify() { command -v notify-send >/dev/null && notify-send -a "Hermes Desktop (Rust)" "$@" 2>/dev/null || true; }
fail() {
  log "FAILED: $*"
  notify -u critical "Update failed" "$* — the current version is still installed. Log: $STATE/update.log"
  exit 1
}

# ── Only follow a Hermes update that succeeded ──
if [ "$after_hermes" = 1 ]; then
  code="$(cat "$HERMES_HOME_DIR/.update_exit_code" 2>/dev/null || echo missing)"
  if [ "$code" != "0" ]; then
    log "last hermes update recorded '$code', not 0 — skipping"
    exit 0
  fi
fi

# ── Fetch ──
command -v cargo >/dev/null || fail "cargo not found (install Rust with rustup)"
if [ ! -d "$SRC/.git" ]; then
  log "cloning $REPO_URL"
  git clone --quiet --branch "$BRANCH" "$REPO_URL" "$SRC" || fail "could not clone $REPO_URL"
fi
git -C "$SRC" fetch --quiet origin "$BRANCH" || fail "could not fetch $BRANCH"
target="$(git -C "$SRC" rev-parse "origin/$BRANCH")"
installed="$(cat "$LIB/COMMIT" 2>/dev/null || true)"
if [ "$force" = 0 ] && [ "$target" = "$installed" ] && [ -x "$LIB/hermes-desktop-rs" ]; then
  log "up to date at ${target:0:7}"
  exit 0
fi
log "updating ${installed:0:7} -> ${target:0:7}"
# A private clone: discard anything in it and check out exactly the fetched commit.
git -C "$SRC" checkout --quiet --force --detach "$target"
git -C "$SRC" clean --quiet -fdx

# ── Build (low priority; the running app is not touched) ──
cd "$SRC"
nice -n 19 ionice -c 3 env CARGO_TARGET_DIR="$DATA/target" \
  cargo build --locked --release -p hermes-app -j "$JOBS" || fail "build of ${target:0:7} failed"
helper_built=0
if [ -d helpers/hermes-browser ]; then
  if (cd helpers/hermes-browser && nice -n 19 ionice -c 3 env CARGO_TARGET_DIR="$DATA/target-browser" \
        cargo build --locked --release -j "$JOBS"); then
    helper_built=1
  else
    log "warning: the browser helper did not build; keeping the installed one"
  fi
fi

# ── Install atomically, keeping the previous version ──
install_bin() {  # <built file> <name>
  local tmp="$LIB/.$2.new"
  install -m755 "$1" "$tmp"
  [ -e "$LIB/$2" ] && cp -f "$LIB/$2" "$LIB/$2.prev"
  mv -f "$tmp" "$LIB/$2"
}
install_bin "$DATA/target/release/hermes-desktop-rs" hermes-desktop-rs
[ "$helper_built" = 1 ] && install_bin "$DATA/target-browser/release/hermes-browser" hermes-browser
# The updater updates itself too, so fixes to this script ship with the app.
[ -f "$SRC/scripts/self-update.sh" ] && install_bin "$SRC/scripts/self-update.sh" hermes-desktop-rs-update
[ -f "$SRC/packaging/hermes-desktop-rs.svg" ] && install -Dm644 "$SRC/packaging/hermes-desktop-rs.svg" \
  "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps/hermes-desktop-rs.svg"
echo "$target" > "$LIB/COMMIT"

subject="$(git -C "$SRC" log -1 --format=%s "$target")"
log "installed ${target:0:7}: $subject"
notify "Updated to ${target:0:7}" "Restart Hermes Desktop (Rust) to use it. $subject"
