#!/usr/bin/env bash
# Writes the GitHub release body to stdout: the hand-written notes for this
# version (docs/releases/v<version>.md) followed by the list of uploaded
# artifacts with per-platform install steps.
#
#   bash scripts/package/release-notes.sh <version> [distdir] > notes.md
set -euo pipefail

version="${1:?usage: release-notes.sh <version> [distdir]}"
distdir="${2:-dist}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
notes="$root/docs/releases/v${version}.md"

if [ -f "$notes" ]; then
  cat "$notes"
else
  cat <<EOF
# Hermes Desktop (Rust) v${version}

Native Rust + Slint desktop client for Hermes Agent. Talks to the **unchanged**
Hermes backend over its JSON-RPC 2.0 WebSocket gateway, the same contract the
Electron desktop and the TUI use.

> No release notes were written for this version. Add them in
> \`docs/releases/v${version}.md\` before tagging (see docs/RELEASING.md); this
> placeholder is generated automatically.
EOF
fi

human_size() {
  awk -v b="$1" 'BEGIN {
    split("B KB MB GB", u, " ");
    i = 1;
    while (b >= 1024 && i < 4) { b /= 1024; i++ }
    printf (i == 1 ? "%.0f %s" : "%.1f %s"), b, u[i]
  }'
}

echo
echo "## Downloads"
echo

shopt -s nullglob
found=0
for file in "$distdir"/*; do
  [ -f "$file" ] || continue
  base="$(basename "$file")"
  size="$(human_size "$(wc -c <"$file")")"
  case "$base" in
    SHA256SUMS.txt)
      echo "- **Checksums** — \`$base\` ($size)"
      echo '  ```sh'
      echo '  sha256sum -c SHA256SUMS.txt'
      echo '  ```'
      ;;
    *.pkg.tar.zst)
      echo "- **Linux — Arch x86_64** — \`$base\` ($size)"
      echo '  ```sh'
      echo "  sudo pacman -U $base"
      echo '  ```'
      echo "  Or build it yourself from a checkout: \`cd packaging && makepkg -si\`."
      ;;
    *windows.zip)
      echo "- **Windows — x86_64** — \`$base\` ($size)"
      echo "  Unzip anywhere and run \`hermes-desktop-rs.exe\`. Needs the Microsoft Visual C++"
      echo "  runtime (installed by most systems) and the \`hermes\` CLI on \`PATH\`."
      ;;
    *macos.dmg)
      echo "- **macOS — universal (Apple silicon + Intel)** — \`$base\` ($size)"
      echo "  Open the disk image, drag **Hermes** to Applications. The build is unsigned:"
      echo "  the first launch needs right-click → **Open** (or System Settings → Privacy &"
      echo "  Security → *Open Anyway*)."
      ;;
    *macos.zip)
      echo "- **macOS — universal, zip** — \`$base\` ($size)"
      echo "  Same bundle as the dmg, for scripted installs: unzip and move \`Hermes.app\` to Applications."
      ;;
    *)
      echo "- \`$base\` ($size)"
      ;;
  esac
  found=$((found + 1))
done

if [ "$found" -eq 0 ]; then
  echo "_No artifacts were attached to this release._"
fi

cat <<'EOF'

## The backend

The client is a front end: it needs the **Hermes Agent backend** (the `hermes`
CLI) available, either already running (`HERMES_DESKTOP_PORT`) or installed so the
app can start `hermes serve`. See the README for the full environment table.

Linux builds target **Arch Linux** only (the packaging is a PKGBUILD); no other
distribution packages are published.
EOF
