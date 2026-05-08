# Reverse of install.ps1 — kills any running daemon, removes the
# Startup shortcut, and deletes the install dir. No registry keys
# to clean up since the per-user installer doesn't touch HKLM.

$ErrorActionPreference = 'Continue'

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\MineShare'
$Startup = [Environment]::GetFolderPath('Startup')
$ShortcutPath = Join-Path $Startup 'MineShare.lnk'

Get-Process -Name 'mineshare-daemon' -ErrorAction SilentlyContinue | ForEach-Object {
    Write-Host "==> stopping running daemon (pid $($_.Id))"
    $_ | Stop-Process -Force
    Start-Sleep -Milliseconds 300
}

if (Test-Path $ShortcutPath) {
    Write-Host "==> removing startup shortcut: $ShortcutPath"
    Remove-Item -Force $ShortcutPath
}

if (Test-Path $InstallDir) {
    Write-Host "==> removing install dir: $InstallDir"
    Remove-Item -Recurse -Force $InstallDir
}

Write-Host ''
Write-Host '==> done.'
