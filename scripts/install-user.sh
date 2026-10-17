#!/usr/bin/env bash
# Install Hermes Desktop (Rust) for the current user, with automatic updates.
#
#   scripts/install-user.sh              install, build the first version, enable updates
#   scripts/install-user.sh --uninstall  remove the app, launcher and update units
#                                        (keeps the build cache in ~/.local/share/hermes-desktop-rs)
#
# Installs to ~/.local/lib/hermes-desktop-rs, adds "Hermes Desktop (Rust)" to the app menu
# and enables two systemd user units: update after every successful `hermes update`, and a
# daily check. Nothing runs as root. See docs/UPDATING.md.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
LIB="$HOME/.local/lib/hermes-desktop-rs"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
UNITS="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
DESKTOP="$APPS/hermes-desktop-rs.desktop"
UNIT_FILES=(hermes-desktop-rs-update.service hermes-desktop-rs-update-after-hermes.service
            hermes-desktop-rs-update.path hermes-desktop-rs-update.timer)

if [ "${1:-}" = "--uninstall" ]; then
  systemctl --user disable --now hermes-desktop-rs-update.path hermes-desktop-rs-update.timer 2>/dev/null || true
  for unit in "${UNIT_FILES[@]}"; do rm -f "$UNITS/$unit"; done
  systemctl --user daemon-reload
  rm -f "$DESKTOP"
  rm -rf "$LIB"
  command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" 2>/dev/null || true
  echo "Removed. Build cache left in ${XDG_DATA_HOME:-$HOME/.local/share}/hermes-desktop-rs (delete it to free space)."
  exit 0
fi

# ── Updater + update units ──
install -Dm755 "$here/scripts/self-update.sh" "$LIB/hermes-desktop-rs-update"
mkdir -p "$UNITS"
for unit in "${UNIT_FILES[@]}"; do install -m644 "$here/packaging/systemd/$unit" "$UNITS/$unit"; done
systemctl --user daemon-reload

# ── Launcher: menus may not have ~/.local/bin on PATH, so pin the hermes CLI found now ──
hermes_bin="$(command -v hermes || true)"
exec_line="$LIB/hermes-desktop-rs"
[ -n "$hermes_bin" ] && exec_line="env HERMES_BIN=$hermes_bin $LIB/hermes-desktop-rs"
mkdir -p "$APPS"
sed -e "s|^Name=.*|Name=Hermes Desktop (Rust)|" \
    -e "s|^Exec=.*|Exec=$exec_line|" \
    "$here/packaging/hermes-desktop-rs.desktop" > "$DESKTOP"
echo "TryExec=$LIB/hermes-desktop-rs" >> "$DESKTOP"
command -v desktop-file-validate >/dev/null && desktop-file-validate "$DESKTOP"
command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" 2>/dev/null || true

# ── First build (the same path every later update takes), then turn updates on ──
"$LIB/hermes-desktop-rs-update" --force
systemctl --user enable --now hermes-desktop-rs-update.path hermes-desktop-rs-update.timer
echo
echo "Installed. Open \"Hermes Desktop (Rust)\" from the app menu."
echo "Updates: after every successful 'hermes update', and daily. Log: ${XDG_STATE_HOME:-$HOME/.local/state}/hermes-desktop-rs/update.log"
