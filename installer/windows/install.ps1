# MineShare — Windows per-user installer.
#
# The GUI embeds the complete bridge runtime. Windows audio, clipboard, and
# input hooks require the interactive user session, so MineShare is launched by
# exactly one per-user scheduled task at logon (never as SYSTEM).

param(
    [string]$BuiltExe,
    [string]$BuiltIcon
)

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $BuiltExe) {
    $BuiltExe = Join-Path $RepoRoot 'ui\src-tauri\target\release\mineshare-app.exe'
}
if (-not $BuiltIcon) {
    $BuiltIcon = Join-Path $RepoRoot 'ui\src-tauri\icons\icon.ico'
}
$InstallDir = Join-Path $env:LOCALAPPDATA 'MineShare'
$InstalledExe = Join-Path $InstallDir 'mineshare-app.exe'
$TaskName = 'MineShareLaunch'
$Startup = [Environment]::GetFolderPath('Startup')
$TaskbarPin = Join-Path $env:APPDATA 'Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar\MineShare.lnk'
$InteractiveUser = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name

if (-not (Test-Path -LiteralPath $BuiltExe)) {
    Write-Error "Built binary not found: $BuiltExe`nBuild first:`n  cd ui; npm run tauri build -- --no-bundle"
}
if (-not (Test-Path -LiteralPath $BuiltIcon)) {
    Write-Error "Built icon not found: $BuiltIcon"
}
$IconHash = (Get-FileHash -LiteralPath $BuiltIcon -Algorithm SHA256).Hash.Substring(0, 12)
$InstalledIcon = Join-Path $InstallDir "mineshare-app-$IconHash.ico"

Get-Process -Name 'mineshare-app', 'mineshare-daemon' -ErrorAction SilentlyContinue |
    ForEach-Object {
        Write-Host "==> stopping $($_.Name) (pid $($_.Id))"
        $_ | Stop-Process -Force
    }
Start-Sleep -Milliseconds 500

Write-Host "==> installing $BuiltExe -> $InstalledExe"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -LiteralPath $BuiltExe -Destination $InstalledExe -Force
Copy-Item -LiteralPath $BuiltIcon -Destination $InstalledIcon -Force

# Create or refresh the stable Taskbar shortcut. Older releases used the
# Programs\MineShare icon path, leaving Explorer's cache stale after an icon
# rebuild. Writing the pin on every deployment also keeps the two-machine
# workflow deterministic if a user profile has lost its shortcut.
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $TaskbarPin) | Out-Null
$ShortcutShell = New-Object -ComObject WScript.Shell
$TaskbarShortcut = $ShortcutShell.CreateShortcut($TaskbarPin)
$TaskbarShortcut.TargetPath = $InstalledExe
$TaskbarShortcut.WorkingDirectory = $InstallDir
$TaskbarShortcut.IconLocation = "$InstalledIcon,0"
$TaskbarShortcut.Save()
Start-Process ie4uinit.exe -ArgumentList '-show' -Wait

# Migration: old releases could start a second, standalone runtime through a
# Startup shortcut or MineShareDaemon task. Remove every known legacy entry.
@(
    (Join-Path $Startup 'MineShare.lnk'),
    (Join-Path $Startup 'MineShare Daemon.lnk'),
    (Join-Path $Startup 'MineShare.daemon.disabled')
) | Where-Object { Test-Path -LiteralPath $_ } | ForEach-Object {
    Write-Host "==> removing legacy startup entry: $_"
    Remove-Item -LiteralPath $_ -Force
}
Unregister-ScheduledTask -TaskName 'MineShareDaemon' -Confirm:$false -ErrorAction SilentlyContinue
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue

Write-Host "==> registering the single per-user logon task: $TaskName"
$Action = New-ScheduledTaskAction -Execute $InstalledExe -WorkingDirectory $InstallDir
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $InteractiveUser
$Principal = New-ScheduledTaskPrincipal `
    -UserId $InteractiveUser `
    -LogonType Interactive `
    -RunLevel Limited
$TaskSettings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -StartWhenAvailable `
    -MultipleInstances IgnoreNew `
    -ExecutionTimeLimit ([TimeSpan]::Zero)
Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $Action `
    -Trigger $Trigger `
    -Principal $Principal `
    -Settings $TaskSettings `
    -Description 'MineShare cross-machine input and audio bridge' |
    Out-Null

Write-Host '==> starting MineShare'
Start-ScheduledTask -TaskName $TaskName

Write-Host ''
Write-Host '==> done.'
Write-Host "  binary:  $InstalledExe"
Write-Host "  startup: scheduled task $TaskName (interactive user)"
Write-Host "  taskbar: $TaskbarPin"
