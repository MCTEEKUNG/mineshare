# MineShare — Windows installer (per-user, no admin required).
#
# Why per-user:
#   The daemon needs the user's interactive session — it installs
#   WH_MOUSE_LL / WH_KEYBOARD_LL hooks, drives WASAPI loopback on
#   the user's audio session, and reads/writes the user's
#   clipboard. A SYSTEM-context Windows Service can do none of
#   those, so we register a per-user logon entry instead.
#
# Layout:
#   Binary:    %LOCALAPPDATA%\Programs\MineShare\mineshare-daemon.exe
#   Autostart: a shortcut in the user's Startup folder pointing at
#              the binary with a minimized-window style. The
#              daemon is a console app, so a tiny window flashes
#              on logon — replacing it with a windows-subsystem
#              build is M5 polish.

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$BuiltExe = Join-Path $RepoRoot 'target\release\mineshare-daemon.exe'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\MineShare'
$InstalledExe = Join-Path $InstallDir 'mineshare-daemon.exe'
$Startup = [Environment]::GetFolderPath('Startup')
$ShortcutPath = Join-Path $Startup 'MineShare.lnk'

if (-not (Test-Path $BuiltExe)) {
    Write-Error "Built binary not found: $BuiltExe`nBuild first:`n  cargo build --release -p mineshare-daemon --bin mineshare-daemon"
}

# Stop a running daemon so we can overwrite the exe.
Get-Process -Name 'mineshare-daemon' -ErrorAction SilentlyContinue | ForEach-Object {
    Write-Host "==> stopping running daemon (pid $($_.Id))"
    $_ | Stop-Process -Force
    Start-Sleep -Milliseconds 300
}

Write-Host "==> installing $BuiltExe -> $InstalledExe"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Force $BuiltExe $InstalledExe

Write-Host "==> creating startup shortcut: $ShortcutPath"
$Shell = New-Object -ComObject WScript.Shell
$Shortcut = $Shell.CreateShortcut($ShortcutPath)
$Shortcut.TargetPath = $InstalledExe
$Shortcut.Arguments = 'run'
$Shortcut.WorkingDirectory = $InstallDir
# 7 = "Minimized, no activate" — daemon runs to tray-equivalent
# without grabbing focus. Won't fully hide the console window,
# but won't pop in front of whatever the user's doing on logon.
$Shortcut.WindowStyle = 7
$Shortcut.Description = 'MineShare cross-machine input + audio bridge'
$Shortcut.Save()

Write-Host "==> starting daemon now"
Start-Process -FilePath $InstalledExe -ArgumentList 'run' -WindowStyle Minimized

Write-Host ''
Write-Host '==> done.'
Write-Host ''
Write-Host "  binary:    $InstalledExe"
Write-Host "  autostart: $ShortcutPath"
Write-Host '  uninstall: powershell -ExecutionPolicy Bypass -File installer\windows\uninstall.ps1'
