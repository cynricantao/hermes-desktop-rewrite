#!/usr/bin/env bash
# Builds the Linux release artifact — Arch only — from this checkout.
#
#   bash scripts/package/linux-arch.sh <version> [outdir]
#
# Produces hermes-desktop-rs-<version>-<pkgrel>-<arch>.pkg.tar.zst via the repo's
# own packaging/PKGBUILD, so the released package is exactly what `makepkg -si`
# builds locally. CI runs this inside an archlinux container as an unprivileged
# user; on Arch it works directly (never run makepkg as root).
set -euo pipefail

version="${1:?usage: linux-arch.sh <version> [outdir]}"
outdir="${2:-dist}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [ "$(id -u)" -eq 0 ]; then
  echo "error: makepkg must not run as root — run this as a normal user" >&2
  exit 1
fi

cd "$root/packaging"

# The tag is the source of truth for the version, so keep pkgver in step with it.
sed -i "s/^pkgver=.*/pkgver=${version}/" PKGBUILD
sed -i "s/^pkgrel=.*/pkgrel=1/" PKGBUILD
echo "== PKGBUILD version lines =="
grep -E '^pkg(ver|rel)=' PKGBUILD

# --syncdeps pulls depends/makedepends (cargo, gtk3, webkit2gtk-4.1, …); the
# PKGBUILD's own check() gate runs the workspace test suite before packaging.
echo "== makepkg =="
makepkg --syncdeps --noconfirm --cleanbuild

mkdir -p "$root/$outdir"
shopt -s nullglob
packages=(*.pkg.tar.zst)
if [ "${#packages[@]}" -eq 0 ]; then
  echo "error: makepkg produced no .pkg.tar.zst" >&2
  exit 1
fi
cp -- "${packages[@]}" "$root/$outdir/"
ls -lh "$root/$outdir"
