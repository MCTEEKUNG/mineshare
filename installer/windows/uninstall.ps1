# MineShare Windows per-user uninstaller.

$ErrorActionPreference = 'Continue'

$InstallDir = Join-Path $env:LOCALAPPDATA 'MineShare'
$LegacyInstallDir = Join-Path $env:LOCALAPPDATA 'Programs\MineShare'
$Startup = [Environment]::GetFolderPath('Startup')

Get-Process -Name 'mineshare-app', 'mineshare-daemon' -ErrorAction SilentlyContinue |
    Stop-Process -Force

Unregister-ScheduledTask -TaskName 'MineShareLaunch' -Confirm:$false -ErrorAction SilentlyContinue
Unregister-ScheduledTask -TaskName 'MineShareDaemon' -Confirm:$false -ErrorAction SilentlyContinue

@(
    (Join-Path $Startup 'MineShare.lnk'),
    (Join-Path $Startup 'MineShare Daemon.lnk'),
    (Join-Path $Startup 'MineShare.daemon.disabled')
) | Where-Object { Test-Path -LiteralPath $_ } | ForEach-Object {
    Remove-Item -LiteralPath $_ -Force
}

# These are fixed, explicitly verified per-user locations; never accept a
# computed or caller-provided recursive deletion target.
foreach ($Path in @($InstallDir, $LegacyInstallDir)) {
    $Full = [IO.Path]::GetFullPath($Path)
    $LocalRoot = [IO.Path]::GetFullPath($env:LOCALAPPDATA)
    if ($Full.StartsWith($LocalRoot, [StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $Full)) {
        Remove-Item -LiteralPath $Full -Recurse -Force
    }
}

Write-Host 'MineShare uninstalled. User settings and logs were preserved.'
