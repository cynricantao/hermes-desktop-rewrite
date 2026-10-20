# Releasing

How a version becomes three downloadable builds plus release notes.

## What gets published

| Platform | Artifact | Built by | Where |
|---|---|---|---|
| Linux (**Arch only**) | `hermes-desktop-rs-<ver>-1-x86_64.pkg.tar.zst` | `makepkg` in an `archlinux` container | `.github/workflows/release.yml` → `linux` job |
| Windows x86_64 | `hermes-desktop-rs-<ver>-x86_64-windows.zip` | `scripts/package/windows.ps1` | → `windows` job |
| macOS universal | `hermes-desktop-rs-<ver>-universal-macos.dmg` + `.zip` | `scripts/package/macos.sh` | → `macos` job |
| all | `SHA256SUMS.txt` | `sha256sum` over the artifacts | → `publish` job |

The Linux build uses the repo's own `packaging/PKGBUILD` (the same file a local
`cd packaging && makepkg -si` uses), so the released package is what the
PKGBUILD says it is. `pkgver` is rewritten from the tag during the build — do
not hand-edit it for a release.

Only Arch is supported on Linux, on purpose: no `.deb`, `.rpm`, Flatpak or
AppImage is produced.

## Cutting a release

1. **Bump the version** in `Cargo.toml` (`[workspace.package] version`). Nothing
   else stores it; the PNG icon and `packaging/PKGBUILD` are version-independent.
2. **Write the notes** in `docs/releases/v<version>.md`. This file becomes the
   release body verbatim. If it is missing, the workflow still publishes, with a
   visible placeholder telling you the notes are missing.
3. **Land it on `main`** (via a normal PR — agents never merge).
4. **Tag and push the tag:**

   ```sh
   git switch main && git pull
   git tag -a v0.1.0 -m "Hermes Desktop (Rust) v0.1.0"
   git push origin v0.1.0
   ```

5. The workflow builds the three platforms, appends the generated **Downloads**
   section, and publishes the release. It fails early if the tag does not match
   the `Cargo.toml` version — that check exists so a bad tag can never produce a
   mislabelled binary.

## Re-publishing / fixing a release

Run the workflow manually (Actions → *Release* → *Run workflow*) with the
existing tag, e.g. `v0.1.0`. It edits that release's notes and re-uploads the
assets with `--clobber` instead of failing on the existing release. Re-running a
tag is the way to recover from a partially published release.

## Local dry runs

The Windows artifact can be built and inspected on a Windows checkout:

```powershell
pwsh -File scripts/package/windows.ps1 -Version 0.1.0
# → dist\hermes-desktop-rs-0.1.0-x86_64-windows.zip
```

On Arch, `bash scripts/package/linux-arch.sh 0.1.0` does locally exactly what CI
does (never as root). The macOS script needs macOS (`lipo`, `iconutil`,
`hdiutil`) and cannot be exercised elsewhere. To preview the release body without
publishing:

```sh
bash scripts/package/release-notes.sh 0.1.0 dist
```

## Known gaps

- **No code signing.** macOS bundles are ad-hoc signed (`codesign --sign -`), and
  Windows has no Authenticode signature. Users get a Gatekeeper/SmartScreen
  prompt; there is no notarization or certificate in the repo.
- **No auto-updater.** The app does not check these releases for updates.
- **No ARM Linux package.** `packaging/PKGBUILD` declares `aarch64`, but CI builds
  x86_64 only.
- **The in-app browser helper ships on Linux only.** `helpers/hermes-browser` is
  a separate GTK/WebKit program built outside the workspace. The Arch package
  builds and installs it (`/usr/bin/hermes-browser`, found next to the app
  binary); the Windows and macOS bundles contain the app alone, so in-app
  browsing is unavailable there.
- **Windows and macOS are built, not tested.** Perf figures in `docs/PERF.md` are
  Linux measurements; nothing is measured on Windows or macOS yet.
