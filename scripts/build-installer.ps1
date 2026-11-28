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

# Use the lockfile for the package manager that maintains this checkout.
# The frontend build command uses npm, and package-lock.json is authoritative
# when both lockfile formats are present.
$useNpm = Test-Path 'package-lock.json'
if ($useNpm) {
    npm ci
} elseif (Test-Path 'bun.lock') {
    bun install --frozen-lockfile
} else {
    bun install
}
if ($LASTEXITCODE -ne 0) { throw 'Frontend dependency installation failed' }

Write-Host ''
Write-Host '==> tauri build' -ForegroundColor Cyan
if ($useNpm) {
    npm run tauri -- build -- --locked
} else {
    bun run tauri build -- --locked
}
if ($LASTEXITCODE -ne 0) { throw 'Tauri build failed' }
node "$Root\installer\windows\build-bootstrapper.mjs"
if ($LASTEXITCODE -ne 0) { throw 'Windows bootstrapper build failed' }

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
