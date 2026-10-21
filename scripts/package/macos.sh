#!/usr/bin/env bash
# Builds the macOS release artifacts from this checkout.
#
#   bash scripts/package/macos.sh <version> [outdir]
#
# Produces a universal (arm64 + x86_64) .app bundle, a .dmg and a .zip:
#   hermes-desktop-rs-<version>-universal-macos.dmg
#   hermes-desktop-rs-<version>-universal-macos.zip
#
# The build is ad-hoc signed (codesign --sign -): it runs, but Gatekeeper still
# warns on first launch because the project has no Developer ID certificate.
# Fonts and icons are compiled into the binary, so the bundle needs no resources
# beyond its icon.
set -euo pipefail

version="${1:?usage: macos.sh <version> [outdir]}"
outdir="${2:-dist}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"

name="hermes-desktop-rs-${version}-universal-macos"
stage="$root/target/package/$name"
app="$stage/Hermes.app"
mkdir -p "$outdir"
rm -rf "$stage"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

echo "== build both architectures =="
rustup target add aarch64-apple-darwin x86_64-apple-darwin
cargo build --release -p hermes-app --target aarch64-apple-darwin
cargo build --release -p hermes-app --target x86_64-apple-darwin

echo "== lipo into a universal binary =="
lipo -create \
  "target/aarch64-apple-darwin/release/hermes-desktop-rs" \
  "target/x86_64-apple-darwin/release/hermes-desktop-rs" \
  -output "$app/Contents/MacOS/hermes-desktop-rs"
chmod 0755 "$app/Contents/MacOS/hermes-desktop-rs"
lipo -info "$app/Contents/MacOS/hermes-desktop-rs"

echo "== Info.plist =="
sed -e "s/@@VERSION@@/${version}/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"

# The app icon ships as ready-made rasters (assets/brand/hermes-icon-<size>.png), so no
# SVG converter is needed; sips (part of macOS) fills any missing size from the 1024 px one.
echo "== app icon =="
brand="$root/assets/brand"
iconset="$stage/AppIcon.iconset"
mkdir -p "$iconset"
# iconutil needs exactly these ten names.
while read -r size file; do
  if [ -f "$brand/hermes-icon-$size.png" ]; then
    cp "$brand/hermes-icon-$size.png" "$iconset/$file"
  else
    sips -z "$size" "$size" "$brand/hermes-icon-1024.png" --out "$iconset/$file" >/dev/null
  fi
done <<'SIZES'
16 icon_16x16.png
32 icon_16x16@2x.png
32 icon_32x32.png
64 icon_32x32@2x.png
128 icon_128x128.png
256 icon_128x128@2x.png
256 icon_256x256.png
512 icon_256x256@2x.png
512 icon_512x512.png
1024 icon_512x512@2x.png
SIZES
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"
echo "icon embedded"
echo "== ad-hoc signature =="
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

echo "== dmg + zip =="
rm -f "$outdir/$name.dmg" "$outdir/$name.zip"
hdiutil create -volname "Hermes ${version}" -srcfolder "$app" -ov -format UDZO "$outdir/$name.dmg"
ditto -c -k --sequesterRsrc --keepParent "$app" "$outdir/$name.zip"
ls -lh "$outdir"
