# Builds the portable Windows release artifact.
#
#   pwsh -File scripts/package/windows.ps1 -Version 0.1.0 [-OutDir dist]
#
# Assumes the MSVC toolchain (rustup default host x86_64-pc-windows-msvc) and a
# release profile build. Produces hermes-desktop-rs-<version>-x86_64-windows.zip
# containing the single self-contained executable plus licences: fonts and icons
# are compiled into the binary (ui/theme.slint, ui/icons.slint), so nothing else
# is needed at runtime.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$OutDir = 'dist'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)   # repo root
if (-not (Test-Path (Join-Path $root 'Cargo.toml'))) {
    throw "repo root not found from $PSScriptRoot"
}

Push-Location $root
try {
    Write-Host "== cargo build --release -p hermes-app =="
    cargo build --release -p hermes-app
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

    $exe = Join-Path $root 'target\release\hermes-desktop-rs.exe'
    if (-not (Test-Path $exe)) { throw "missing $exe" }

    $name = "hermes-desktop-rs-$Version-x86_64-windows"
    $stage = Join-Path $root "target\package\$name"
    if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
    New-Item -ItemType Directory -Path $stage -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $stage 'licenses') -Force | Out-Null

    Copy-Item $exe (Join-Path $stage 'hermes-desktop-rs.exe')
    foreach ($file in 'LICENSE', 'README.md') {
        $src = Join-Path $root $file
        if (Test-Path $src) { Copy-Item $src (Join-Path $stage $file) }
    }
    # Fonts and the icon set are OFL/ISC; their licence texts ship next to them.
    Get-ChildItem (Join-Path $root 'assets\fonts') -Filter '*-OFL.txt' -File |
        ForEach-Object { Copy-Item $_.FullName (Join-Path $stage 'licenses') }
    Copy-Item (Join-Path $root 'assets\icons\LICENSE-lucide.txt') (Join-Path $stage 'licenses')

    $out = if ([System.IO.Path]::IsPathRooted($OutDir)) { $OutDir } else { Join-Path $root $OutDir }
    New-Item -ItemType Directory -Path $out -Force | Out-Null
    $zip = Join-Path $out "$name.zip"
    if (Test-Path $zip) { Remove-Item $zip -Force }
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal

    $size = [math]::Round((Get-Item $zip).Length / 1MB, 1)
    Write-Host "built $zip ($size MB)"
    Get-ChildItem $stage -Recurse -File | ForEach-Object {
        Write-Host ("  {0}  {1}" -f $_.FullName.Substring($stage.Length + 1), $_.Length)
    }
}
finally {
    Pop-Location
}
