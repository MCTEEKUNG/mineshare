# Build all distributable installers for Windows.
#
# Produces:
#   * .msi  (WiX-based, traditional installer)
#   * -setup.exe  (NSIS-based, smaller download)
#
# Tauri's bundler can't cross-compile, so .deb / .AppImage need
# to be built on Linux via scripts/build-installer.sh (or via
# the .github/workflows/release.yml GitHub Action).

$ErrorActionPreference = 'Stop'

$Root = Resolve-Path "$PSScriptRoot\.."
Set-Location "$Root\ui"

# Frozen lockfile so `bun install` doesn't silently bump deps
# on a release build. Falls back to plain install for first-
# time checkouts.
if (Test-Path 'bun.lock') {
    bun install --frozen-lockfile
} else {
    bun install
}

Write-Host ''
Write-Host '==> tauri build' -ForegroundColor Cyan
bun run tauri build

Write-Host ''
Write-Host '==> Installer artifacts:' -ForegroundColor Cyan
$bundleDir = 'src-tauri\target\release\bundle'
if (Test-Path $bundleDir) {
    Get-ChildItem $bundleDir -Recurse -Include *.msi, *-setup.exe |
        ForEach-Object {
            $sizeMB = [math]::Round($_.Length / 1MB, 1)
            Write-Host ('  {0}  ({1} MB)' -f $_.FullName, $sizeMB)
        }
} else {
    Write-Host '  (no bundle dir found - check tauri build output above)' -ForegroundColor Yellow
}
